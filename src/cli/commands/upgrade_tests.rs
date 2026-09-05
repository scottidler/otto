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
    /// One byte every `0` interval, forever, under a `Content-Length` it never
    /// reaches. The between-bytes read timeout never fires because bytes keep
    /// arriving; only a whole-transfer budget can end this.
    Dribble(Duration),
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
        Some(Route::Dribble(interval)) => {
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: 1048576\r\nConnection: close\r\n\r\n"
            );
            let _ = stream.flush();
            loop {
                if stream.write_all(b"x").is_err() || stream.flush().is_err() {
                    return;
                }
                std::thread::sleep(*interval);
            }
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
        api_base: None,
        install_target: None,
        current_version: None,
    }
}

/// ETXTBSY is reproducible on Linux: hold a write descriptor open on an
/// executable and exec fails with errno 26 for as long as the handle lives.
///
/// This pins the two things the retry ceiling change is for: the wait is bounded
/// by wall clock and not by an attempt count, and when the budget expires the
/// error says what happened. The previous code returned a bare
/// "Failed to execute new binary" after an unmeasured 500ms.
///
/// Linux only, not `unix`: XNU does not set the text-busy bit from an ordinary
/// write descriptor, so on macOS the exec succeeds and the test's own
/// precondition is absent. It fails loudly rather than passing vacuously,
/// which is correct, and the honest answer is that the platform has nothing
/// here to test.
#[cfg(target_os = "linux")]
#[test]
fn a_binary_held_open_for_writing_fails_within_the_budget_and_names_the_cause() {
    use std::io::Write;

    let temp = TempDir::new().expect("tempdir");
    let binary = temp.path().join("otto");
    // A real, runnable executable, so the only reason exec can fail is the
    // descriptor the test is about to hold.
    let mut handle = fs::File::create(&binary).expect("create");
    handle.write_all(b"#!/bin/sh\nexit 0\n").expect("write");
    handle.flush().expect("flush");
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).expect("chmod");

    // `handle` is still open for writing here. That is what produces ETXTBSY.
    let upgrader = command();
    let budget = Duration::from_millis(300);
    let started = std::time::Instant::now();
    let result = upgrader.verify_binary_within(&binary, budget);
    let elapsed = started.elapsed();

    // If this platform did not actually produce ETXTBSY the test proves nothing,
    // so say so rather than passing vacuously.
    let Err(err) = result else {
        drop(handle);
        panic!("expected ETXTBSY while a write descriptor was open; exec succeeded instead");
    };
    let message = format!("{err:#}");

    assert!(
        message.contains("held open for writing by"),
        "the error must explain ETXTBSY rather than restating errno: {message}"
    );
    assert!(
        message.contains(&binary.display().to_string()),
        "the error must name the binary it could not run: {message}"
    );
    assert!(
        message.contains("Nothing was installed"),
        "the error must tell the user their current otto is untouched: {message}"
    );

    // Bounded by the budget, not by an attempt count. The generous upper bound is
    // process-spawn overhead per attempt, which is not what is being asserted;
    // the point is that it terminates near the budget rather than at some
    // attempt-count-derived time unrelated to it.
    assert!(
        elapsed >= budget.mul_f32(0.5),
        "gave up far too early: {elapsed:?} against a {budget:?} budget"
    );
    assert!(
        elapsed < budget * 8,
        "the budget was not enforced: {elapsed:?} against a {budget:?} budget"
    );

    // Releasing the descriptor makes the same exec succeed, which proves the
    // descriptor was the cause and the binary itself was always fine.
    drop(handle);
    upgrader
        .verify_binary_within(&binary, Duration::from_secs(2))
        .expect("the same binary must verify once the write handle is closed");
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
    let started = std::time::Instant::now();
    loop {
        match Command::new(path).arg("--version").output() {
            Ok(output) => {
                assert!(output.status.success(), "installed binary failed --version");
                return String::from_utf8_lossy(&output.stdout).trim().to_string();
            }
            Err(err)
                if cfg!(unix)
                    && err.raw_os_error() == Some(ETXTBSY)
                    && started.elapsed() + VERIFY_RETRY_DELAY <= VERIFY_BUSY_BUDGET =>
            {
                std::thread::sleep(VERIFY_RETRY_DELAY);
            }
            Err(err) => panic!("run installed binary after {:?}: {err}", started.elapsed()),
        }
    }
}

#[tokio::test]
async fn download_and_check_checksum_keeps_the_archive_alive_after_the_call() {
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
        .download_and_check_checksum(&client, &format!("{}/otto.tar.gz", base))
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
async fn download_and_check_checksum_rejects_a_mismatch() {
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
        .download_and_check_checksum(&client, &format!("{}/otto.tar.gz", base))
        .await
        .expect_err("a mismatched checksum must abort");

    assert!(
        err.to_string().contains("Checksum mismatch"),
        "unexpected error: {}",
        err
    );
}

#[tokio::test]
async fn download_and_check_checksum_rejects_a_missing_sibling() {
    let scratch = tempfile::tempdir().unwrap();
    let tarball = fixture_tarball(scratch.path(), "9.9.9");

    let mut routes = HashMap::new();
    routes.insert("/otto.tar.gz".to_string(), Route::Body(tarball));
    let base = spawn_stub(routes);

    let client = build_http_client(CONNECT_TIMEOUT, READ_TIMEOUT).unwrap();
    let err = command()
        .download_and_check_checksum(&client, &format!("{}/otto.tar.gz", base))
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

/// A server that keeps sending, just far too slowly.
///
/// This is the case the between-bytes `READ_TIMEOUT` structurally cannot catch:
/// every chunk resets it, so the transfer runs forever. Before the whole-transfer
/// budget existed this was measured past 150 seconds and had no upper bound at
/// all. The test dribbles fast enough that the read timeout is never close to
/// firing, so the only thing that can end the run is the budget.
#[tokio::test]
async fn a_download_that_dribbles_forever_is_ended_by_the_whole_transfer_budget() {
    let mut routes = HashMap::new();
    routes.insert("/otto.tar.gz".to_string(), Route::Dribble(Duration::from_millis(10)));
    let base = spawn_stub(routes);

    // Read timeout an order of magnitude longer than the dribble interval: if
    // this test passes because of the read timeout rather than the budget, that
    // is a bug in the test and this is what rules it out.
    let client = build_http_client(CONNECT_TIMEOUT, Duration::from_secs(5)).unwrap();
    let budget = Duration::from_millis(500);

    let started = SystemTime::now();
    let err = command()
        .download_with_progress_within(&client, &format!("{}/otto.tar.gz", base), budget)
        .await
        .expect_err("a download that never finishes must be ended by the budget");
    let elapsed = started.elapsed().unwrap();

    let message = format!("{err:#}");
    assert!(
        message.contains("exceeded its"),
        "the error must say the budget was exceeded, not blame the connection: {message}"
    );
    assert!(
        message.contains("Nothing was installed"),
        "the error must tell the user their current otto is untouched: {message}"
    );
    // Bounded by the budget, and by a margin well under the read timeout, which
    // proves the budget is what ended it.
    assert!(
        elapsed >= budget,
        "ended before the budget was spent: {elapsed:?} against {budget:?}"
    );
    assert!(
        elapsed < Duration::from_secs(3),
        "the budget was not enforced: {elapsed:?} against {budget:?}"
    );
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
        .download_and_check_checksum(&client, &format!("{}/otto.tar.gz", base))
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
#[serial_test::serial]
fn test_backup_dir_default() {
    let cmd = command();

    // `$OTTO_HOME` is what decides, so the test says which one it is rather
    // than reading whatever the process happens to have. Serialized because the
    // environment is process-global and other tests in this binary set it too.
    let restore = std::env::var("OTTO_HOME").ok();
    // SAFETY: `#[serial]` keeps this the only thread touching the environment.
    unsafe { std::env::set_var("OTTO_HOME", "/tmp/otto-home-fixture") };
    let with_otto_home = cmd.get_backup_dir().unwrap();
    unsafe { std::env::remove_var("OTTO_HOME") };
    let without_otto_home = cmd.get_backup_dir().unwrap();
    if let Some(value) = restore {
        // SAFETY: as above.
        unsafe { std::env::set_var("OTTO_HOME", value) };
    }

    assert_eq!(
        with_otto_home,
        PathBuf::from("/tmp/otto-home-fixture/backups"),
        "backups belong under the otto home, not under a rebuilt $HOME/.otto"
    );
    assert!(
        without_otto_home.to_string_lossy().ends_with(".otto/backups"),
        "unexpected default: {}",
        without_otto_home.display()
    );
}

#[test]
fn test_backup_dir_custom() {
    let custom_path = PathBuf::from("/tmp/custom-backups");
    let cmd = UpgradeCommand {
        backup_dir: Some(custom_path.clone()),
        ..command()
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

/// The JSON object GitHub returns for one published release.
fn release_json(version: &str, platform: &str, download_base: &str) -> String {
    format!(
        r#"{{"tag_name":"v{version}","name":"v{version}","published_at":"2026-01-01T00:00:00Z",
            "assets":[{{"name":"otto-v{version}-{platform}.tar.gz",
                        "browser_download_url":"{download_base}/otto.tar.gz"}}]}}"#
    )
}

/// One published release: a tarball and checksum on their own stub, plus the
/// JSON that points at them.
struct FixtureRelease {
    version: String,
    json: String,
}

/// Publish one fixture release. Its own stub serves the archive, because the
/// asset URL inside the release JSON is absolute and a stub's port is not known
/// until it is bound. `digest_override` replaces the published checksum, for the
/// mismatch case.
fn fixture_release(scratch: &Path, version: &str, digest_override: Option<String>) -> FixtureRelease {
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

    FixtureRelease {
        version: version.to_string(),
        json: release_json(version, &platform, &download_base),
    }
}

/// Serve the three release endpoints `Upgrade` reads and return the API base
/// `with_fixture` takes.
///
/// `latest` answers `/releases/latest`. `listing` answers `/releases` in the
/// order given, which is GitHub's creation order and includes prereleases.
/// `tagged` answers `/releases/tags/v<version>` and is deliberately its own
/// list: a release too old for page 1 of the listing is reachable by tag and
/// not by listing, which is the case `--version` used to fail on.
fn spawn_release_api(latest: &FixtureRelease, listing: &[&FixtureRelease], tagged: &[&FixtureRelease]) -> String {
    let mut routes = HashMap::new();
    routes.insert(
        "/releases/latest".to_string(),
        Route::Body(latest.json.clone().into_bytes()),
    );

    let array: Vec<&str> = listing.iter().map(|r| r.json.as_str()).collect();
    routes.insert(
        "/releases".to_string(),
        Route::Body(format!("[{}]", array.join(",")).into_bytes()),
    );

    for release in tagged {
        routes.insert(
            format!("/releases/tags/v{}", release.version),
            Route::Body(release.json.clone().into_bytes()),
        );
    }

    spawn_stub(routes)
}

/// The single-release case: one version, reachable every way.
fn spawn_fixture_release(scratch: &Path, version: &str, digest_override: Option<String>) -> String {
    let release = fixture_release(scratch, version, digest_override);
    spawn_release_api(&release, &[&release], &[&release])
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
/// `download_and_check_checksum` + `install_from_archive`, skipping `current_version`,
/// the already-on-target and downgrade short-circuits, `find_asset` and
/// `create_backup` - it asserted its own transcription of the command body
/// rather than the command. Found by the batched audit, batch 6 of 14.
#[tokio::test]
async fn execute_installs_a_fixture_release_end_to_end() {
    let scratch = tempfile::tempdir().expect("tempdir");
    let api_base = spawn_fixture_release(scratch.path(), "9.9.9", None);
    let target = installed_binary(scratch.path(), "0.0.1");
    assert_eq!(run_version(&target), "otto 0.0.1");

    command()
        .with_fixture(api_base, &target)
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
    let api_base = spawn_fixture_release(scratch.path(), "9.9.9", Some("a".repeat(64)));
    let target = installed_binary(scratch.path(), "0.0.1");

    let err = command()
        .with_fixture(api_base, &target)
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

/// A staging file abandoned by a killed upgrade is reaped by the next one.
///
/// `commit_staged` cleans up on a returned error, which is every failure the
/// process survives; a signal between the copy and the rename is not one of
/// those, and nothing reaped the result. Found by the batched audit, batch 6 of
/// 14, which measured 10 SIGKILL trials leaving a stranded file each time.
#[tokio::test]
async fn a_stale_staging_file_from_a_dead_pid_is_reaped() {
    let scratch = tempfile::tempdir().expect("tempdir");
    let api_base = spawn_fixture_release(scratch.path(), "9.9.9", None);
    let target = installed_binary(scratch.path(), "0.0.1");
    let dir = target.parent().expect("bin parent");

    // pid 1 is always live; a very high pid is not. Both shapes present, so the
    // test proves the reaper discriminates rather than deleting everything.
    let abandoned = dir.join(".otto.upgrade-4294967290");
    let live = dir.join(".otto.upgrade-1");
    fs::write(&abandoned, "dead process left this").expect("write abandoned");
    fs::write(&live, "a running process owns this").expect("write live");

    command()
        .with_fixture(api_base, &target)
        .tap_no_backup()
        .execute()
        .await
        .expect("install must succeed");

    assert!(!abandoned.exists(), "a staging file from a dead pid must be reaped");
    assert!(
        live.exists(),
        "a staging file whose pid is still running must be left alone"
    );
    assert_eq!(run_version(&target), "otto 9.9.9");
}

/// `--help` must never render the value of `GITHUB_TOKEN`.
///
/// clap renders an env-backed arg as `[env: VAR=<value>]` by default, so
/// `otto Upgrade --help` printed the user's live token to stdout - into terminal
/// scrollback, CI logs, and any transcript of a help invocation. Observed during
/// the audit against a real `ghp_`-prefixed token on this machine. Found by the
/// batched audit, batch 9 of 14.
#[test]
fn help_never_renders_the_github_token_value() {
    use clap::CommandFactory;

    const SENTINEL: &str = "ghp_SENTINEL_DO_NOT_PRINT_123456";
    // SAFETY: single-threaded test process; the var is removed immediately after.
    unsafe { std::env::set_var("GITHUB_TOKEN", SENTINEL) };
    let help = UpgradeCommand::command().render_long_help().to_string();
    unsafe { std::env::remove_var("GITHUB_TOKEN") };

    assert!(!help.contains(SENTINEL), "--help leaked the token value:\n{help}");
    assert!(
        help.contains("github-token"),
        "the flag itself must still be documented:\n{help}"
    );
}

/// `pid_is_running` decides whether a `.otto.upgrade-<pid>` file is abandoned,
/// so it has to answer a pid, not a process group. The reaper's own fixture
/// uses 4294967290, which is exactly the value that casts to -6 and turns the
/// question into "is process group 6 alive". This pins both ends.
///
/// It also pins the platform: the old implementation read `/proc/<pid>` and
/// returned `true` for everything on non-Linux, so macOS never reaped a file.
#[test]
fn pid_is_running_answers_a_pid_and_never_a_process_group() {
    assert!(
        pid_is_running(std::process::id()),
        "this very process must read as running"
    );
    assert!(pid_is_running(1), "pid 1 must read as running");

    assert!(
        !pid_is_running(4294967290),
        "a pid that cannot fit a positive pid_t must read as not running, \
         not be cast into a negative process-group id"
    );
    assert!(
        !pid_is_running(0),
        "pid 0 means the caller's own process group to kill(2); it is never a pid to reap against"
    );
}

/// The default target is the release GitHub marks latest, not the newest entry
/// on page 1 of the listing.
///
/// `/releases` is creation order and includes prereleases, so its first entry is
/// whatever was cut most recently - the 9.9.10 prerelease here. `install.sh` has
/// always read `/releases/latest`; this is `otto Upgrade` agreeing with it.
#[tokio::test]
async fn the_default_target_comes_from_the_latest_endpoint_not_the_listing() {
    let scratch = tempfile::tempdir().expect("tempdir");
    let prerelease = fixture_release(scratch.path(), "9.9.10", None);
    let latest = fixture_release(scratch.path(), "9.9.9", None);
    // Creation order: the prerelease was cut last, so it is listed first.
    let api_base = spawn_release_api(&latest, &[&prerelease, &latest], &[&prerelease, &latest]);
    let target = installed_binary(scratch.path(), "0.0.1");

    command()
        .with_fixture(api_base, &target)
        .tap_no_backup()
        .execute()
        .await
        .expect("execute() must complete the install");

    assert_eq!(
        run_version(&target),
        "otto 9.9.9",
        "the default target must be the release GitHub calls latest, not the newest-created one"
    );
}

/// `--version X` asks for the tag by name, so a release too old to be on page 1
/// of the listing installs instead of reporting "Release vX not found".
#[tokio::test]
async fn an_explicit_version_is_fetched_by_tag_even_when_the_listing_omits_it() {
    let scratch = tempfile::tempdir().expect("tempdir");
    let latest = fixture_release(scratch.path(), "9.9.9", None);
    let off_page = fixture_release(scratch.path(), "9.9.8", None);
    // The listing knows nothing about 9.9.8; only its tag route serves it.
    let api_base = spawn_release_api(&latest, &[&latest], &[&latest, &off_page]);
    let target = installed_binary(scratch.path(), "0.0.1");

    let mut cmd = command().with_fixture(api_base, &target).tap_no_backup();
    cmd.version = Some("9.9.8".to_string());
    cmd.execute().await.expect("execute() must install the tagged release");

    assert_eq!(run_version(&target), "otto 9.9.8");
}

/// A version that no release carries is still a named error.
#[tokio::test]
async fn an_unknown_explicit_version_names_the_release_it_could_not_find() {
    let scratch = tempfile::tempdir().expect("tempdir");
    let latest = fixture_release(scratch.path(), "9.9.9", None);
    let api_base = spawn_release_api(&latest, &[&latest], &[&latest]);
    let target = installed_binary(scratch.path(), "0.0.1");

    let mut cmd = command().with_fixture(api_base, &target).tap_no_backup();
    cmd.version = Some("1.2.3".to_string());
    let err = cmd.execute().await.expect_err("an unpublished version must fail");

    assert!(
        format!("{err:#}").contains("Release v1.2.3 not found"),
        "the error must name the version, got: {err:#}"
    );
    assert_eq!(run_version(&target), "otto 0.0.1", "the binary must be untouched");
}

/// Rolling back twice walks the versions down instead of undoing itself.
///
/// A rollback writes a safety backup of the binary it replaces, so that file is
/// the newest entry in the directory when the next rollback runs. Choosing "the
/// newest backup" therefore restored the version the previous rollback had just
/// left. Three versions in play, two rollbacks, and the binary must end on the
/// oldest.
#[tokio::test]
async fn two_rollbacks_walk_down_to_the_oldest_backup() {
    let scratch = tempfile::tempdir().expect("tempdir");
    let backup_dir = scratch.path().join("backups");
    fs::create_dir_all(&backup_dir).expect("create backup dir");

    for (version, timestamp) in [("1.0.0", 100), ("2.0.0", 200)] {
        let backup = backup_dir.join(format!("otto-{version}-{timestamp}.backup"));
        fs::write(&backup, format!("#!/bin/sh\necho \"otto {version}\"\n")).expect("write backup");
        fs::set_permissions(&backup, fs::Permissions::from_mode(0o755)).expect("chmod backup");
    }

    let target = installed_binary(scratch.path(), "3.0.0");

    // Each rollback is a separate process running the binary the previous one
    // installed. `with_current_version` is how that is said: one process's own
    // version is fixed at build time.
    let mut first = command()
        .with_fixture("http://127.0.0.1:1/unused", &target)
        .with_current_version("3.0.0");
    first.rollback = true;
    first.backup_dir = Some(backup_dir.clone());
    first.execute().await.expect("first rollback must succeed");
    assert_eq!(
        run_version(&target),
        "otto 2.0.0",
        "the first rollback goes back one version"
    );

    // The safety backup of 3.0.0 is now the newest file in the directory. It is
    // the trap: "the newest backup" would restore the version just left.
    let safety_backup_present = fs::read_dir(&backup_dir)
        .expect("read backup dir")
        .flatten()
        .any(|e| e.file_name().to_string_lossy().starts_with("otto-3.0.0-"));
    assert!(
        safety_backup_present,
        "the first rollback must have backed up the version it replaced"
    );

    let mut second = command()
        .with_fixture("http://127.0.0.1:1/unused", &target)
        .with_current_version("2.0.0");
    second.rollback = true;
    second.backup_dir = Some(backup_dir.clone());
    second.execute().await.expect("second rollback must succeed");

    assert_eq!(
        run_version(&target),
        "otto 1.0.0",
        "the second rollback must go back another version, not undo the first"
    );
}

/// The safety backup is written once per version, not once per rollback.
#[tokio::test]
async fn a_rollback_does_not_stack_a_second_backup_of_the_version_it_is_leaving() {
    let scratch = tempfile::tempdir().expect("tempdir");
    let backup_dir = scratch.path().join("backups");
    fs::create_dir_all(&backup_dir).expect("create backup dir");

    for (version, timestamp) in [("1.0.0", 100), ("3.0.0", 300)] {
        let backup = backup_dir.join(format!("otto-{version}-{timestamp}.backup"));
        fs::write(&backup, format!("#!/bin/sh\necho \"otto {version}\"\n")).expect("write backup");
        fs::set_permissions(&backup, fs::Permissions::from_mode(0o755)).expect("chmod backup");
    }

    let target = installed_binary(scratch.path(), "3.0.0");

    let mut cmd = command()
        .with_fixture("http://127.0.0.1:1/unused", &target)
        .with_current_version("3.0.0");
    cmd.rollback = true;
    cmd.backup_dir = Some(backup_dir.clone());
    cmd.execute().await.expect("rollback must succeed");

    assert_eq!(run_version(&target), "otto 1.0.0");
    let backups = cmd.list_backups().expect("list backups");
    assert_eq!(
        backups.len(),
        2,
        "v3.0.0 was already backed up, so no third file belongs here: {backups:?}"
    );
}

#[test]
fn rollback_refuses_a_backup_that_is_not_older_than_the_current_version() {
    let backups = vec![
        BackupInfo {
            path: PathBuf::from("/backups/otto-3.0.0-300.backup"),
            version: "3.0.0".to_string(),
            timestamp: 300,
        },
        BackupInfo {
            path: PathBuf::from("/backups/otto-2.0.0-200.backup"),
            version: "2.0.0".to_string(),
            timestamp: 200,
        },
    ];

    // The newest backup is the safety backup of the version running now, and
    // the other one is that same version's predecessor... which is what makes
    // "newest wins" oscillate. Older wins instead.
    let chosen = select_rollback_target(&backups, "3.0.0").expect("2.0.0 is older");
    assert_eq!(chosen.version, "2.0.0");

    // Nothing older at all is an error naming what is there, not a silent
    // reinstall of the current version.
    let err = select_rollback_target(&backups, "2.0.0").expect_err("nothing older than 2.0.0");
    let message = format!("{err:#}");
    assert!(message.contains("No backup older than the current v2.0.0"), "{message}");
    assert!(
        message.contains("v3.0.0"),
        "the error must list what is present: {message}"
    );
}

#[test]
fn rollback_skips_a_backup_whose_version_is_not_a_semver() {
    let backups = vec![
        BackupInfo {
            path: PathBuf::from("/backups/otto-nightly-400.backup"),
            version: "nightly".to_string(),
            timestamp: 400,
        },
        BackupInfo {
            path: PathBuf::from("/backups/otto-1.0.0-100.backup"),
            version: "1.0.0".to_string(),
            timestamp: 100,
        },
    ];

    // `~/.otto/backups` is a directory anything can write into, so an
    // unorderable name is skipped rather than assumed to be older.
    let chosen = select_rollback_target(&backups, "2.0.0").expect("1.0.0 is older");
    assert_eq!(chosen.version, "1.0.0");
}

/// `git describe` past a tag names a build NEWER than that tag. Semver reads
/// `2.2.1-27-g85bb8fb` as a PRERELEASE of 2.2.1, which sorts below the release,
/// so every ordering built on `Version::parse` alone went the wrong way for a
/// dev build.
#[test]
fn a_dev_build_orders_above_the_tag_it_descends_from() {
    let dev = parse_build_version("v2.2.1-27-g85bb8fb").expect("a describe string is orderable");
    let release = parse_build_version("2.2.1").expect("a tag is orderable");
    let next = parse_build_version("2.2.2").expect("a tag is orderable");

    assert!(dev > release, "27 commits past v2.2.1 is newer than v2.2.1, not older");
    assert!(dev < next, "and still older than the next release");

    // A genuine prerelease has no `-<n>-g<sha>` suffix, so it keeps semver's
    // own ordering: rc.1 comes before the release it is a candidate for.
    let rc = parse_build_version("2.3.0-rc.1").expect("a prerelease tag is orderable");
    assert!(rc < parse_build_version("2.3.0").expect("a tag is orderable"));
}

/// `git describe --tags --always` falls back to a bare sha in a checkout with
/// no reachable tag, which used to reach the user as semver's own
/// `unexpected character` with no hint of where the string came from.
#[test]
fn a_bare_sha_is_refused_with_a_reason() {
    let err = parse_build_version("85bb8fb").expect_err("a sha is not a version");
    let message = format!("{err:#}");
    assert!(message.contains("bare commit sha"), "{message}");
}

#[test]
fn rollback_refuses_a_dev_build_that_is_newer_than_the_current_version() {
    let backups = vec![
        BackupInfo {
            path: PathBuf::from("/backups/otto-2.2.1-27-g85bb8fb-500.backup"),
            version: "2.2.1-27-g85bb8fb".to_string(),
            timestamp: 500,
        },
        BackupInfo {
            path: PathBuf::from("/backups/otto-2.1.0-100.backup"),
            version: "2.1.0".to_string(),
            timestamp: 100,
        },
    ];

    // The newest backup is a build 27 commits PAST the version running now.
    // Read as a semver prerelease it looked older, so rollback restored a newer
    // binary and called it a rollback.
    let chosen = select_rollback_target(&backups, "2.2.1").expect("2.1.0 is older");
    assert_eq!(chosen.version, "2.1.0");
}

/// The same ordering, through `execute()`: a dev build is not upgraded "up" to
/// the release it descends from.
#[tokio::test]
async fn execute_refuses_to_downgrade_a_dev_build_to_its_own_base_tag() {
    let scratch = tempfile::tempdir().expect("tempdir");
    let api_base = spawn_fixture_release(scratch.path(), "2.2.1", None);
    let target = installed_binary(scratch.path(), "0.0.1");

    command()
        .with_fixture(api_base, &target)
        .with_current_version("2.2.1-27-gcfc60e6")
        .tap_no_backup()
        .execute()
        .await
        .expect("a refused downgrade is not an error");

    assert_eq!(
        run_version(&target),
        "otto 0.0.1",
        "v2.2.1-27-gcfc60e6 is newer than v2.2.1; installing it is a downgrade and needs --force"
    );
}

/// A staging copy that cannot be made executable takes its own file with it.
///
/// The copy has already succeeded at that point, so the old `?` returned and
/// left `.otto.upgrade-<pid>` beside the binary until some later upgrade's
/// reaper found it.
#[test]
fn a_staged_copy_is_removed_when_it_cannot_be_made_executable() {
    let scratch = tempfile::tempdir().expect("tempdir");
    let target = scratch.path().join("otto");
    fs::write(&target, "#!/bin/sh\necho \"otto 0.0.1\"\n").expect("write target");
    fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).expect("chmod target");

    let replacement = scratch.path().join("new-otto");
    fs::write(&replacement, "#!/bin/sh\necho \"otto 9.9.9\"\n").expect("write replacement");

    let err = stage_beside_with(&replacement, &target, |_| Err(eyre!("chmod refused")))
        .expect_err("a failing permission step must fail the staging");
    assert!(format!("{err:#}").contains("chmod refused"), "unexpected: {err:#}");

    let debris: Vec<String> = fs::read_dir(scratch.path())
        .expect("read scratch dir")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.starts_with(".otto.upgrade-"))
        .collect();
    assert!(debris.is_empty(), "staging debris left behind: {debris:?}");
    assert_eq!(run_version(&target), "otto 0.0.1", "the target must be untouched");
}

/// The `--dry-run` plan names the file the real run downloads.
///
/// The plan printed `otto-{version}-{platform}.tar.gz` while `find_asset` looked
/// for `otto-v{version}-...`, so the plan named a file no release publishes.
#[test]
fn the_dry_run_plan_names_the_asset_find_asset_looks_for() {
    let platform = PlatformInfo::detect().expect("detect platform");
    let published = asset_name("9.9.9", &platform.platform_str);

    let steps = command()
        .dry_run_steps("9.9.9", &platform)
        .expect("dry-run steps must render");
    assert!(
        steps[0].contains(&published),
        "step 1 must name the published asset {published}, got: {}",
        steps[0]
    );

    // ...and that is exactly the name `find_asset` accepts.
    let release = GitHubRelease {
        tag_name: "v9.9.9".to_string(),
        published_at: "2026-01-01T00:00:00Z".to_string(),
        assets: vec![GitHubAsset {
            name: published.clone(),
            browser_download_url: "http://example.invalid/a.tar.gz".to_string(),
        }],
    };
    assert_eq!(
        command()
            .find_asset(&release, &platform.platform_str)
            .expect("find asset")
            .name,
        published
    );

    // The spelling the plan used to print is not a published asset.
    let mispublished = GitHubRelease {
        tag_name: "v9.9.9".to_string(),
        published_at: "2026-01-01T00:00:00Z".to_string(),
        assets: vec![GitHubAsset {
            name: format!("otto-9.9.9-{}.tar.gz", platform.platform_str),
            browser_download_url: "http://example.invalid/a.tar.gz".to_string(),
        }],
    };
    assert!(
        command().find_asset(&mispublished, &platform.platform_str).is_err(),
        "the old dry-run spelling must not be mistaken for a published asset"
    );
}

/// The plan is a promise about the order the real run works in. It listed the
/// backup before the checksum while `execute_upgrade` verifies the download
/// first, so a reader who hit a checksum failure went looking for a backup that
/// was never written.
#[test]
fn the_dry_run_plan_checks_the_checksum_before_it_makes_a_backup() {
    let platform = PlatformInfo::detect().expect("detect platform");

    let steps = command()
        .dry_run_steps("9.9.9", &platform)
        .expect("dry-run steps must render");

    let checksum = steps
        .iter()
        .position(|s| s.contains("checksum"))
        .unwrap_or_else(|| panic!("the plan must name the checksum step: {steps:?}"));
    let backup = steps
        .iter()
        .position(|s| s.contains("Create backup"))
        .unwrap_or_else(|| panic!("the plan must name the backup step: {steps:?}"));

    assert!(
        checksum < backup,
        "execute_upgrade verifies the download before it backs anything up: {steps:?}"
    );
}

/// Every step in the plan is numbered from its position, so `--no-backup`
/// cannot print "1, 3, 4, 5" again.
#[test]
fn the_dry_run_plan_drops_the_backup_step_without_leaving_a_gap() {
    let platform = PlatformInfo::detect().expect("detect platform");

    let with_backup = command()
        .dry_run_steps("9.9.9", &platform)
        .expect("dry-run steps must render");
    assert!(
        with_backup.iter().any(|s| s.contains("Create backup")),
        "{with_backup:?}"
    );

    let without_backup = command()
        .tap_no_backup()
        .dry_run_steps("9.9.9", &platform)
        .expect("dry-run steps must render");
    assert!(
        !without_backup.iter().any(|s| s.contains("Create backup")),
        "{without_backup:?}"
    );
    assert_eq!(
        without_backup.len(),
        with_backup.len() - 1,
        "only the backup step may disappear: {without_backup:?}"
    );

    // The numbering the plan prints is the position in this list, so it is
    // contiguous by construction; this pins the render that relies on it.
    let rendered: Vec<String> = without_backup
        .iter()
        .enumerate()
        .map(|(i, step)| format!("  {}. {}", i + 1, step))
        .collect();
    assert!(rendered[1].starts_with("  2. "), "{rendered:?}");
}

/// The "(latest)" marker names the release GitHub calls latest, not whichever
/// release was created most recently.
#[test]
fn the_latest_marker_follows_the_latest_endpoint_not_the_listing_order() {
    let releases = vec![
        GitHubRelease {
            tag_name: "v9.9.10".to_string(),
            published_at: "2026-02-02T00:00:00Z".to_string(),
            assets: Vec::new(),
        },
        GitHubRelease {
            tag_name: "v9.9.9".to_string(),
            published_at: "2026-01-01T00:00:00Z".to_string(),
            assets: Vec::new(),
        },
    ];

    let lines = command().version_lines(&releases, Some("v9.9.9"));
    assert!(!lines[0].contains("(latest)"), "{lines:?}");
    assert!(lines[1].contains("(latest)"), "{lines:?}");
    assert!(lines[1].contains("2026-01-01"), "{lines:?}");

    // No answer from `/releases/latest` means no marker, not a guess.
    let unmarked = command().version_lines(&releases, None);
    assert!(!unmarked.iter().any(|l| l.contains("(latest)")), "{unmarked:?}");
}
