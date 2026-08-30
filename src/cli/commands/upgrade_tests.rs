#![cfg(test)]

use super::*;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;
use std::sync::Arc;

/// What the stub server does with a request for a given path.
enum Route {
    Body(Vec<u8>),
    Stall,
}

/// Minimal HTTP/1.1 stub bound to an ephemeral localhost port. It lets the
/// upgrade path be exercised through the real reqwest client without a
/// network and without a fixture release on GitHub.
fn spawn_stub(routes: HashMap<String, Route>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind stub server");
    let base = format!("http://{}", listener.local_addr().expect("stub addr"));
    let routes = Arc::new(routes);

    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let routes = Arc::clone(&routes);
            std::thread::spawn(move || serve(stream, &routes));
        }
    });

    base
}

fn serve(mut stream: std::net::TcpStream, routes: &HashMap<String, Route>) {
    let Some(path) = read_request_path(&stream) else {
        return;
    };

    match routes.get(&path) {
        Some(Route::Body(body)) => {
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(body);
        }
        Some(Route::Stall) => {
            // Headers promise a body that never arrives: the classic stall.
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: 4096\r\nConnection: close\r\n\r\n"
            );
            let _ = stream.flush();
            std::thread::sleep(Duration::from_secs(10));
        }
        None => {
            let _ = write!(
                stream,
                "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
        }
    }

    let _ = stream.flush();
}

fn read_request_path(stream: &std::net::TcpStream) -> Option<String> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line).ok()?;

    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).ok()? == 0 || header == "\r\n" || header == "\n" {
            break;
        }
    }

    request_line.split_whitespace().nth(1).map(|p| p.to_string())
}

/// Build a release tarball holding an executable `otto` that reports
/// `version` when run with `--version`.
fn fixture_tarball(scratch: &Path, version: &str) -> Vec<u8> {
    let staging = scratch.join(format!("fixture-{}", version));
    fs::create_dir_all(&staging).expect("fixture staging dir");

    let binary = staging.join("otto");
    fs::write(&binary, format!("#!/bin/sh\necho \"otto {}\"\n", version)).expect("write fixture binary");
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).expect("chmod fixture binary");

    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    {
        let mut builder = tar::Builder::new(&mut encoder);
        builder.append_path_with_name(&binary, "otto").expect("append fixture");
        builder.finish().expect("finish tar");
    }

    encoder.finish().expect("finish gzip")
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn command() -> UpgradeCommand {
    UpgradeCommand {
        dry_run: false,
        version: None,
        list_versions: false,
        rollback: false,
        force: false,
        no_backup: false,
        backup_dir: None,
        github_token: None,
        releases_url: None,
        install_target: None,
    }
}

/// Run a just-written binary's `--version`, retrying on ETXTBSY.
///
/// These tests write an executable and immediately exec it. Under a
/// parallel `cargo test`, another thread's `fork` can momentarily inherit
/// the still-open write descriptor, and the exec fails with ETXTBSY
/// (`ExecutableFileBusy`) even though the file is complete and correct.
/// Production hits the same race, which is why `verify_binary` retries on
/// exactly this errno; this helper needs the same treatment for the same
/// reason. Without it `rollback_restores_the_previous_binary` flakes under
/// load - observed twice in this plan's own phase runs, each time passing
/// on rerun and in isolation.
fn run_version(path: &Path) -> String {
    for attempt in 1..=VERIFY_ATTEMPTS {
        match Command::new(path).arg("--version").output() {
            Ok(output) => {
                assert!(output.status.success(), "installed binary failed --version");
                return String::from_utf8_lossy(&output.stdout).trim().to_string();
            }
            Err(err) if cfg!(unix) && err.raw_os_error() == Some(ETXTBSY) && attempt < VERIFY_ATTEMPTS => {
                std::thread::sleep(VERIFY_RETRY_DELAY);
            }
            Err(err) => panic!("run installed binary: {err}"),
        }
    }
    unreachable!("run_version exhausted retries without returning or panicking")
}

#[tokio::test]
async fn download_and_verify_keeps_the_archive_alive_after_the_call() {
    let scratch = tempfile::tempdir().unwrap();
    let tarball = fixture_tarball(scratch.path(), "9.9.9");

    let mut routes = HashMap::new();
    routes.insert("/otto.tar.gz".to_string(), Route::Body(tarball.clone()));
    routes.insert(
        "/otto.tar.gz.sha256".to_string(),
        Route::Body(format!("{}  otto.tar.gz\n", sha256_hex(&tarball)).into_bytes()),
    );
    let base = spawn_stub(routes);

    let client = build_http_client(CONNECT_TIMEOUT, READ_TIMEOUT).unwrap();
    let archive = command()
        .download_and_verify(&client, &format!("{}/otto.tar.gz", base))
        .await
        .expect("download and verify");

    // The bug this phase opens with: the archive used to be deleted the
    // moment the download call returned.
    assert!(archive.path().exists(), "archive must outlive the download call");
    assert_eq!(fs::read(archive.path()).unwrap(), tarball);

    let path = archive.path().to_path_buf();
    drop(archive);
    assert!(!path.exists(), "dropping the handle must clean the archive up");
}

#[tokio::test]
async fn download_and_verify_rejects_a_checksum_mismatch() {
    let scratch = tempfile::tempdir().unwrap();
    let tarball = fixture_tarball(scratch.path(), "9.9.9");

    let mut routes = HashMap::new();
    routes.insert("/otto.tar.gz".to_string(), Route::Body(tarball));
    routes.insert(
        "/otto.tar.gz.sha256".to_string(),
        Route::Body(format!("{}  otto.tar.gz\n", "0".repeat(64)).into_bytes()),
    );
    let base = spawn_stub(routes);

    let client = build_http_client(CONNECT_TIMEOUT, READ_TIMEOUT).unwrap();
    let err = command()
        .download_and_verify(&client, &format!("{}/otto.tar.gz", base))
        .await
        .expect_err("a mismatched checksum must abort");

    assert!(
        err.to_string().contains("Checksum mismatch"),
        "unexpected error: {}",
        err
    );
}

#[tokio::test]
async fn download_and_verify_rejects_a_missing_checksum_sibling() {
    let scratch = tempfile::tempdir().unwrap();
    let tarball = fixture_tarball(scratch.path(), "9.9.9");

    let mut routes = HashMap::new();
    routes.insert("/otto.tar.gz".to_string(), Route::Body(tarball));
    let base = spawn_stub(routes);

    let client = build_http_client(CONNECT_TIMEOUT, READ_TIMEOUT).unwrap();
    let err = command()
        .download_and_verify(&client, &format!("{}/otto.tar.gz", base))
        .await
        .expect_err("a missing checksum must abort");

    assert!(
        err.to_string().contains("Checksum file not available"),
        "unexpected error: {}",
        err
    );
}

#[tokio::test]
async fn download_rejects_a_non_success_status() {
    let base = spawn_stub(HashMap::new());

    let client = build_http_client(CONNECT_TIMEOUT, READ_TIMEOUT).unwrap();
    let err = command()
        .download_with_progress(&client, &format!("{}/otto.tar.gz", base))
        .await
        .expect_err("a 404 must not be treated as an archive");

    assert!(err.to_string().contains("Download failed"), "unexpected error: {}", err);
}

#[tokio::test]
async fn download_times_out_when_the_stream_stalls() {
    let mut routes = HashMap::new();
    routes.insert("/otto.tar.gz".to_string(), Route::Stall);
    let base = spawn_stub(routes);

    let client = build_http_client(CONNECT_TIMEOUT, Duration::from_millis(300)).unwrap();
    let started = SystemTime::now();
    let err = command()
        .download_with_progress(&client, &format!("{}/otto.tar.gz", base))
        .await
        .expect_err("a stalled stream must time out");

    let elapsed = started.elapsed().unwrap();
    assert!(elapsed < Duration::from_secs(5), "timed out too late: {:?}", elapsed);
    assert!(
        err.to_string().contains("Download interrupted"),
        "unexpected error: {}",
        err
    );
}

#[tokio::test]
async fn upgrade_installs_a_fixture_release_end_to_end() {
    let scratch = tempfile::tempdir().unwrap();
    let tarball = fixture_tarball(scratch.path(), "9.9.9");

    let mut routes = HashMap::new();
    routes.insert("/otto.tar.gz".to_string(), Route::Body(tarball.clone()));
    routes.insert(
        "/otto.tar.gz.sha256".to_string(),
        Route::Body(format!("{}  otto.tar.gz\n", sha256_hex(&tarball)).into_bytes()),
    );
    let base = spawn_stub(routes);

    // The install target is a scratch path, never the running executable.
    let target = scratch.path().join("bin").join("otto");
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(&target, "#!/bin/sh\necho \"otto 0.0.1\"\n").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(run_version(&target), "otto 0.0.1");

    let client = build_http_client(CONNECT_TIMEOUT, READ_TIMEOUT).unwrap();
    let cmd = command();
    let archive = cmd
        .download_and_verify(&client, &format!("{}/otto.tar.gz", base))
        .await
        .expect("download and verify");
    cmd.install_from_archive(archive.path(), &target).expect("install");

    assert_eq!(run_version(&target), "otto 9.9.9");
    assert_eq!(
        fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o755,
        "installed binary must be executable"
    );
    let leftovers: Vec<_> = fs::read_dir(target.parent().unwrap())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n != "otto")
        .collect();
    assert!(leftovers.is_empty(), "staging debris left behind: {:?}", leftovers);
}

#[test]
fn install_from_archive_leaves_the_target_intact_when_the_binary_is_missing() {
    let scratch = tempfile::tempdir().unwrap();

    // An archive with no `otto` entry inside it.
    let stray = scratch.path().join("README");
    fs::write(&stray, "not a binary").unwrap();
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    {
        let mut builder = tar::Builder::new(&mut encoder);
        builder.append_path_with_name(&stray, "README").unwrap();
        builder.finish().unwrap();
    }
    let archive_path = scratch.path().join("otto.tar.gz");
    fs::write(&archive_path, encoder.finish().unwrap()).unwrap();

    let target = scratch.path().join("otto");
    fs::write(&target, "#!/bin/sh\necho \"otto 0.0.1\"\n").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();

    let err = command()
        .install_from_archive(&archive_path, &target)
        .expect_err("an archive without otto must not install");

    assert!(err.to_string().contains("not found in archive"), "unexpected: {}", err);
    assert_eq!(run_version(&target), "otto 0.0.1", "target must be untouched");
}

#[test]
fn staging_leaves_the_target_untouched_until_the_rename() {
    let scratch = tempfile::tempdir().unwrap();

    let target = scratch.path().join("otto");
    fs::write(&target, "#!/bin/sh\necho \"otto 0.0.1\"\n").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();

    let replacement = scratch.path().join("new-otto");
    fs::write(&replacement, "#!/bin/sh\necho \"otto 9.9.9\"\n").unwrap();

    // Stage only: this is the window an interrupted upgrade dies in.
    let staged = stage_beside(&replacement, &target).expect("stage");
    assert_ne!(staged, target);
    assert_eq!(staged.parent(), target.parent(), "staging must share the target's dir");
    assert_eq!(
        run_version(&target),
        "otto 0.0.1",
        "an interrupted upgrade must leave the original intact and executable"
    );

    // Completing the rename is what makes the swap visible.
    commit_staged(&staged, &target).expect("commit");
    assert_eq!(run_version(&target), "otto 9.9.9");
    assert!(!staged.exists(), "staged file must be consumed by the rename");
}

#[test]
fn rollback_restores_the_previous_binary() {
    let scratch = tempfile::tempdir().unwrap();
    let backup_dir = scratch.path().join("backups");
    fs::create_dir_all(&backup_dir).unwrap();

    for (version, timestamp, reported) in [("0.5.5", 100, "0.5.5"), ("0.5.6", 200, "0.5.6")] {
        let backup = backup_dir.join(format!("otto-{}-{}.backup", version, timestamp));
        fs::write(&backup, format!("#!/bin/sh\necho \"otto {}\"\n", reported)).unwrap();
        fs::set_permissions(&backup, fs::Permissions::from_mode(0o755)).unwrap();
    }

    let target = scratch.path().join("otto");
    fs::write(&target, "#!/bin/sh\necho \"otto 9.9.9\"\n").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();

    let mut cmd = command();
    cmd.backup_dir = Some(backup_dir);

    let backups = cmd.list_backups().expect("list backups");
    assert_eq!(backups[0].version, "0.5.6", "newest backup must come first");

    cmd.verify_binary(&backups[0].path).expect("verify backup");
    install_binary(&backups[0].path, &target).expect("restore");

    assert_eq!(run_version(&target), "otto 0.5.6");
}

#[test]
fn parse_sha256_manifest_reads_the_sha256sum_format() {
    let digest = "a".repeat(64);
    let parsed = parse_sha256_manifest(&format!("{}  otto-v1.0.0-linux.tar.gz\n", digest.to_uppercase()));
    assert_eq!(parsed.unwrap(), digest);
}

#[test]
fn parse_sha256_manifest_rejects_a_non_digest() {
    for body in ["", "not-a-digest  otto.tar.gz", &"a".repeat(63)] {
        assert!(
            parse_sha256_manifest(body).is_err(),
            "expected rejection for {:?}",
            body
        );
    }
}

#[test]
fn sha256_file_matches_a_known_digest() {
    let scratch = tempfile::tempdir().unwrap();
    let path = scratch.path().join("payload");
    fs::write(&path, b"abc").unwrap();

    assert_eq!(
        sha256_file(&path).unwrap(),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[tokio::test]
async fn fetch_releases_rejects_a_non_success_status() {
    let base = spawn_stub(HashMap::new());

    let client = build_http_client(CONNECT_TIMEOUT, READ_TIMEOUT).unwrap();
    let err = command()
        .fetch_releases(&client, &format!("{}/releases", base))
        .await
        .expect_err("a 404 release listing must be an error");

    assert!(err.to_string().contains("404"), "unexpected error: {}", err);
}

#[tokio::test]
async fn fetch_releases_parses_a_release_listing() {
    let body = r#"[{"tag_name":"v9.9.9","name":"v9.9.9","published_at":"2026-01-02T03:04:05Z",
            "assets":[{"name":"otto-v9.9.9-linux-amd64.tar.gz","browser_download_url":"http://example.invalid/a.tar.gz"}]}]"#;

    let mut routes = HashMap::new();
    routes.insert("/releases".to_string(), Route::Body(body.as_bytes().to_vec()));
    let base = spawn_stub(routes);

    let client = build_http_client(CONNECT_TIMEOUT, READ_TIMEOUT).unwrap();
    let cmd = command();
    let releases = cmd
        .fetch_releases(&client, &format!("{}/releases", base))
        .await
        .expect("parse releases");

    assert_eq!(releases[0].tag_name, "v9.9.9");
    let asset = cmd.find_asset(&releases[0], "linux-amd64").expect("find asset");
    assert_eq!(asset.name, "otto-v9.9.9-linux-amd64.tar.gz");
}

#[test]
fn platform_strings_match_the_published_asset_suffixes() {
    // install.sh get_suffix() and .github/workflows/release-and-publish.yml
    // are the source of truth; a drift here makes Upgrade look for a
    // tarball no release contains.
    let install_sh = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/install.sh")).unwrap();

    for suffix in ["linux-amd64", "linux-arm64", "macos-x86_64", "macos-arm64"] {
        assert!(
            install_sh.contains(&format!("echo \"{}\"", suffix)),
            "install.sh no longer publishes {}",
            suffix
        );
    }

    let detected = PlatformInfo::detect().unwrap().platform_str;
    assert!(
        install_sh.contains(&format!("echo \"{}\"", detected)),
        "detected platform {:?} is not a published asset suffix",
        detected
    );
}

#[test]
fn test_platform_detection() {
    let platform = PlatformInfo::detect().unwrap();
    assert!(!platform.platform_str.is_empty());
    assert!(
        platform._os == "linux" || platform._os == "macos",
        "Unexpected OS: {}",
        platform._os
    );
}

#[test]
fn test_version_parsing() {
    let v1 = Version::parse("0.5.5").unwrap();
    let v2 = Version::parse("0.5.6").unwrap();
    assert!(v1 < v2);
}

#[test]
fn test_backup_dir_default() {
    let cmd = UpgradeCommand {
        dry_run: false,
        version: None,
        list_versions: false,
        rollback: false,
        force: false,
        no_backup: false,
        backup_dir: None,
        github_token: None,
        releases_url: None,
        install_target: None,
    };

    let backup_dir = cmd.get_backup_dir().unwrap();
    assert!(backup_dir.to_string_lossy().contains(".otto/backups"));
}

#[test]
fn test_backup_dir_custom() {
    let custom_path = PathBuf::from("/tmp/custom-backups");
    let cmd = UpgradeCommand {
        dry_run: false,
        version: None,
        list_versions: false,
        rollback: false,
        force: false,
        no_backup: false,
        backup_dir: Some(custom_path.clone()),
        github_token: None,
        releases_url: None,
        install_target: None,
    };

    let backup_dir = cmd.get_backup_dir().unwrap();
    assert_eq!(backup_dir, custom_path);
}

/// `list_backups` reads a directory that anything could have written into
/// (it's just `~/.otto/backups`, or `--backup-dir`), so its `otto-VERSION-
/// TIMESTAMP.backup` parser must not panic on a name that does not fit that
/// shape. None of these should crash; only the well-formed one should
/// surface as a `BackupInfo`.
#[test]
fn list_backups_tolerates_adversarial_filenames() {
    let temp = tempfile::tempdir().unwrap();
    let backup_dir = temp.path().join("backups");
    fs::create_dir_all(&backup_dir).unwrap();

    // Names that must not panic, and must not be mistaken for a real backup.
    let excluded: &[&str] = &[
        "otto-solo.backup",           // no hyphen to split version from timestamp
        "not-otto-shaped-at-all.txt", // doesn't even carry the prefix
        "otto-latest.backup",         // the symlink name itself, without a version-timestamp body
    ];
    for name in excluded {
        fs::write(backup_dir.join(name), b"not a real binary").unwrap();
    }
    // A non-numeric timestamp is accepted, not rejected: `.unwrap_or(0)`
    // degrades to timestamp 0 (sorts last) rather than dropping the entry.
    fs::write(backup_dir.join("otto-1.2.3-notanumber.backup"), b"not a real binary").unwrap();
    // One well-formed entry, so the parser's success path is exercised too.
    fs::write(backup_dir.join("otto-1.4.0-1700000000.backup"), b"binary bytes").unwrap();

    let cmd = UpgradeCommand {
        backup_dir: Some(backup_dir),
        ..command()
    };

    // Must not panic; must find exactly the two entries whose names carry a
    // version-timestamp body, and none of the excluded ones.
    let backups = cmd
        .list_backups()
        .expect("list_backups must not error on adversarial names");
    assert_eq!(backups.len(), 2, "{backups:?}");
    assert_eq!(backups[0].version, "1.4.0", "newest first: {backups:?}");
    assert_eq!(backups[0].timestamp, 1700000000);
    assert_eq!(backups[1].version, "1.2.3", "{backups:?}");
    assert_eq!(
        backups[1].timestamp, 0,
        "an unparseable timestamp degrades to 0, it does not panic"
    );
}

/// A 200 response whose body is not valid JSON at all must be a named
/// error, not a panic - `fetch_releases_rejects_a_non_success_status`
/// covers the HTTP-status half of this; this is the body-shape half.
#[tokio::test]
async fn fetch_releases_rejects_malformed_json() {
    let mut routes = HashMap::new();
    routes.insert(
        "/releases".to_string(),
        Route::Body(b"this is not json at all {{{".to_vec()),
    );
    let base = spawn_stub(routes);

    let client = build_http_client(CONNECT_TIMEOUT, READ_TIMEOUT).unwrap();
    let err = command()
        .fetch_releases(&client, &format!("{}/releases", base))
        .await
        .expect_err("a malformed release listing must be an error, not a panic");

    assert!(!err.to_string().is_empty());
}

/// Same shape, but the body is valid JSON of the *wrong* shape (an object,
/// not the expected array of releases).
#[tokio::test]
async fn fetch_releases_rejects_a_json_object_where_an_array_was_expected() {
    let mut routes = HashMap::new();
    routes.insert(
        "/releases".to_string(),
        Route::Body(br#"{"message":"Not Found"}"#.to_vec()),
    );
    let base = spawn_stub(routes);

    let client = build_http_client(CONNECT_TIMEOUT, READ_TIMEOUT).unwrap();
    let err = command()
        .fetch_releases(&client, &format!("{}/releases", base))
        .await
        .expect_err("a JSON object where an array was expected must be an error, not a panic");

    assert!(!err.to_string().is_empty());
}

/// Build the releases JSON GitHub would return for one published version.
fn releases_json(version: &str, platform: &str, download_base: &str) -> Vec<u8> {
    format!(
        r#"[{{"tag_name":"v{version}","name":"v{version}","published_at":"2026-01-01T00:00:00Z",
            "assets":[{{"name":"otto-v{version}-{platform}.tar.gz",
                        "browser_download_url":"{download_base}/otto.tar.gz"}}]}}]"#
    )
    .into_bytes()
}

/// Two stubs, because the asset URL inside the releases JSON is absolute and the
/// stub's port is not known until it is bound: one host serves the archive and
/// its checksum, the other serves the release metadata pointing at the first.
/// `digest_override` replaces the published checksum, for the mismatch case.
fn spawn_fixture_release(scratch: &Path, version: &str, digest_override: Option<String>) -> String {
    let platform = PlatformInfo::detect().expect("detect platform").platform_str;
    let tarball = fixture_tarball(scratch, version);
    let digest = digest_override.unwrap_or_else(|| sha256_hex(&tarball));

    let mut download_routes = HashMap::new();
    download_routes.insert("/otto.tar.gz".to_string(), Route::Body(tarball));
    download_routes.insert(
        "/otto.tar.gz.sha256".to_string(),
        Route::Body(format!("{digest}  otto.tar.gz\n").into_bytes()),
    );
    let download_base = spawn_stub(download_routes);

    let mut meta_routes = HashMap::new();
    meta_routes.insert(
        "/releases".to_string(),
        Route::Body(releases_json(version, &platform, &download_base)),
    );
    let meta_base = spawn_stub(meta_routes);

    format!("{meta_base}/releases")
}

/// Write a stand-in "installed otto" that reports `version`.
fn installed_binary(dir: &Path, version: &str) -> PathBuf {
    let target = dir.join("bin").join("otto");
    fs::create_dir_all(target.parent().expect("bin parent")).expect("create bin dir");
    fs::write(&target, format!("#!/bin/sh\necho \"otto {version}\"\n")).expect("write installed binary");
    fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).expect("chmod installed binary");
    target
}

/// `UpgradeCommand::execute()` itself installs a fixture release.
///
/// The phase's success criterion is "`otto Upgrade` completes an install
/// end-to-end against a fixture release". The test that claimed it hand-composed
/// `download_and_verify` + `install_from_archive`, skipping `current_version`,
/// the already-on-target and downgrade short-circuits, `find_asset` and
/// `create_backup` - it asserted its own transcription of the command body
/// rather than the command. Found by the batched audit, batch 6 of 14.
#[tokio::test]
async fn execute_installs_a_fixture_release_end_to_end() {
    let scratch = tempfile::tempdir().expect("tempdir");
    let releases_url = spawn_fixture_release(scratch.path(), "9.9.9", None);
    let target = installed_binary(scratch.path(), "0.0.1");
    assert_eq!(run_version(&target), "otto 0.0.1");

    command()
        .with_fixture(releases_url, &target)
        .tap_no_backup()
        .execute()
        .await
        .expect("execute() must complete the install");

    assert_eq!(
        run_version(&target),
        "otto 9.9.9",
        "execute() must have replaced the installed binary"
    );

    let leftovers: Vec<String> = fs::read_dir(target.parent().expect("bin parent"))
        .expect("read bin dir")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n != "otto")
        .collect();
    assert!(leftovers.is_empty(), "staging debris left behind: {leftovers:?}");
}

/// `execute()` refuses to install a release whose checksum does not match, and
/// leaves the installed binary untouched.
#[tokio::test]
async fn execute_aborts_on_a_checksum_mismatch_without_touching_the_binary() {
    let scratch = tempfile::tempdir().expect("tempdir");
    // Well-formed digest, wrong tarball.
    let releases_url = spawn_fixture_release(scratch.path(), "9.9.9", Some("a".repeat(64)));
    let target = installed_binary(scratch.path(), "0.0.1");

    let err = command()
        .with_fixture(releases_url, &target)
        .tap_no_backup()
        .execute()
        .await
        .expect_err("a checksum mismatch must abort");

    assert!(
        err.to_string().contains("Checksum mismatch"),
        "the error must name the mismatch, got: {err}"
    );
    assert_eq!(
        run_version(&target),
        "otto 0.0.1",
        "the installed binary must survive a checksum mismatch"
    );
}

/// `execute()` with `--rollback` restores the previous binary.
///
/// The doc's rollback criterion says rollback "is asserted, not assumed" because
/// this phase changes rollback behavior. The test that claimed it hand-copied
/// `execute_rollback`'s three steps, so a reorder that rolled back onto itself
/// would not have turned it red. Found by the batched audit, batch 6 of 14.
#[tokio::test]
async fn execute_rollback_restores_the_previous_binary_end_to_end() {
    let scratch = tempfile::tempdir().expect("tempdir");
    let target = installed_binary(scratch.path(), "9.9.9");

    // A backup of the version we want back.
    let backup_dir = scratch.path().join("backups");
    fs::create_dir_all(&backup_dir).expect("create backup dir");
    let backup = backup_dir.join("otto-0.0.1-1700000000.backup");
    fs::write(&backup, "#!/bin/sh\necho \"otto 0.0.1\"\n").expect("write backup");
    fs::set_permissions(&backup, fs::Permissions::from_mode(0o755)).expect("chmod backup");

    assert_eq!(run_version(&target), "otto 9.9.9");

    let mut cmd = command()
        .with_fixture("http://127.0.0.1:1/unused", &target)
        .tap_no_backup();
    cmd.rollback = true;
    cmd.backup_dir = Some(backup_dir);
    cmd.execute().await.expect("rollback must succeed");

    assert_eq!(
        run_version(&target),
        "otto 0.0.1",
        "execute(--rollback) must restore the backed-up binary"
    );
}

/// Rolling back with no backups present must fail loudly and leave the binary
/// alone, rather than reporting success having done nothing.
#[tokio::test]
async fn execute_rollback_with_no_backups_fails_and_leaves_the_binary() {
    let scratch = tempfile::tempdir().expect("tempdir");
    let target = installed_binary(scratch.path(), "9.9.9");
    let backup_dir = scratch.path().join("backups");
    fs::create_dir_all(&backup_dir).expect("create backup dir");

    let mut cmd = command()
        .with_fixture("http://127.0.0.1:1/unused", &target)
        .tap_no_backup();
    cmd.rollback = true;
    cmd.backup_dir = Some(backup_dir);
    let err = cmd.execute().await.expect_err("rollback with no backups must fail");

    assert!(
        err.to_string().contains("No backups found"),
        "the error must say there is nothing to roll back to, got: {err}"
    );
    assert_eq!(run_version(&target), "otto 9.9.9", "the binary must be untouched");
}
