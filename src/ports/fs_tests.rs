#![cfg(test)]

use super::*;

#[tokio::test]
async fn test_memfs_write_and_read() {
    let fs = MemFs::new();
    let path = Path::new("/tmp/test.txt");

    fs.write(path, b"hello world").await.unwrap();
    let content = fs.read_to_string(path).await.unwrap();

    assert_eq!(content, "hello world");
}

#[test]
fn realfs_write_sync_replaces_the_file_and_leaves_no_debris() {
    let temp_dir = tempfile::tempdir().unwrap();
    let path = temp_dir.path().join("script.sh");
    let fs = RealFs;

    fs.write_sync(&path, b"first version, longer").unwrap();
    fs.write_sync(&path, b"second").unwrap();

    assert_eq!(fs.read_sync(&path).unwrap(), b"second");
    // The staging file is renamed, never left behind next to the target.
    let entries: Vec<_> = std::fs::read_dir(temp_dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(entries.len(), 1, "expected only the target file, got {entries:?}");
}

#[test]
fn realfs_read_sync_reports_a_missing_file() {
    let temp_dir = tempfile::tempdir().unwrap();
    let fs = RealFs;
    assert!(fs.read_sync(&temp_dir.path().join("nope")).is_err());
}

#[tokio::test]
async fn test_memfs_exists() {
    let fs = MemFs::new();
    let path = Path::new("/tmp/test.txt");

    assert!(!fs.exists(path).await);

    fs.write(path, b"content").await.unwrap();

    assert!(fs.exists(path).await);
}

#[tokio::test]
async fn test_memfs_is_dir() {
    let fs = MemFs::new();
    let dir = Path::new("/tmp/mydir");
    let file = Path::new("/tmp/file.txt");

    fs.create_dir_all(dir).await.unwrap();
    fs.write(file, b"content").await.unwrap();

    assert!(fs.is_dir(dir).await);
    assert!(!fs.is_dir(file).await);
}

#[tokio::test]
async fn test_memfs_metadata() {
    let fs = MemFs::new();
    let path = Path::new("/tmp/test.txt");

    fs.write(path, b"hello").await.unwrap();
    let meta = fs.metadata(path).await.unwrap();

    assert_eq!(meta.len, 5);
    assert!(meta.is_file);
    assert!(!meta.is_dir);
}

#[tokio::test]
async fn test_memfs_copy() {
    let fs = MemFs::new();
    let src = Path::new("/tmp/src.txt");
    let dst = Path::new("/tmp/dst.txt");

    fs.write(src, b"copy me").await.unwrap();
    let bytes = fs.copy(src, dst).await.unwrap();

    assert_eq!(bytes, 7);
    assert_eq!(fs.read_to_string(dst).await.unwrap(), "copy me");
}

#[tokio::test]
async fn test_memfs_remove() {
    let fs = MemFs::new();
    let path = Path::new("/tmp/test.txt");

    fs.write(path, b"content").await.unwrap();
    assert!(fs.exists(path).await);

    fs.remove_file(path).await.unwrap();
    assert!(!fs.exists(path).await);
}

#[tokio::test]
async fn test_memfs_read_dir() {
    let fs = MemFs::new();
    let dir = Path::new("/tmp/mydir");

    fs.write(&dir.join("file1.txt"), b"content1").await.unwrap();
    fs.write(&dir.join("file2.txt"), b"content2").await.unwrap();
    fs.create_dir_all(&dir.join("subdir")).await.unwrap();

    let entries = fs.read_dir(dir).await.unwrap();
    assert_eq!(entries.len(), 3);
}

#[tokio::test]
async fn test_realfs_temp_file() {
    let fs = RealFs;
    let temp_dir = tempfile::tempdir().unwrap();
    let path = temp_dir.path().join("test.txt");

    fs.write(&path, b"test content").await.unwrap();
    assert!(fs.exists(&path).await);

    let content = fs.read_to_string(&path).await.unwrap();
    assert_eq!(content, "test content");

    let meta = fs.metadata(&path).await.unwrap();
    assert!(meta.is_file);
    assert_eq!(meta.len, 12);
}

#[tokio::test]
async fn test_memfs_symlink() {
    let fs = MemFs::new();
    let original = Path::new("/tmp/original.txt");
    let link = Path::new("/tmp/link.txt");

    fs.write(original, b"original content").await.unwrap();
    fs.symlink(original, link).await.unwrap();

    let target = fs.read_link(link).await.unwrap();
    assert_eq!(target, original);
}

#[tokio::test]
async fn test_memfs_canonicalize() {
    let fs = MemFs::new();
    let path = Path::new("/tmp/test.txt");

    // Path doesn't exist yet
    assert!(fs.canonicalize(path).await.is_err());

    // Create file
    fs.write(path, b"content").await.unwrap();

    // Now canonicalize should work
    let canonical = fs.canonicalize(path).await.unwrap();
    assert_eq!(canonical, path);
}

#[tokio::test]
async fn test_memfs_set_permissions() {
    let fs = MemFs::new();
    let path = Path::new("/tmp/test.txt");

    fs.write(path, b"content").await.unwrap();

    // set_permissions is a no-op for MemFs, but should succeed
    fs.set_permissions(path, 0o755).await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn test_realfs_symlink() {
    let fs = RealFs;
    let temp_dir = tempfile::tempdir().unwrap();
    let original = temp_dir.path().join("original.txt");
    let link = temp_dir.path().join("link.txt");

    fs.write(&original, b"original content").await.unwrap();
    fs.symlink(&original, &link).await.unwrap();

    let target = fs.read_link(&link).await.unwrap();
    assert_eq!(target, original);
}

// Sync method tests for MemFs
#[test]
fn test_memfs_exists_sync() {
    let fs = MemFs::new();
    let path = Path::new("/tmp/test.txt");

    assert!(!fs.exists_sync(path));

    fs.write_sync(path, b"content").unwrap();

    assert!(fs.exists_sync(path));
}

#[test]
fn test_memfs_write_and_read_sync() {
    let fs = MemFs::new();
    let path = Path::new("/tmp/test.txt");

    fs.write_sync(path, b"hello world").unwrap();

    // Verify parent directories were created
    assert!(fs.exists_sync(Path::new("/tmp")));
}

#[test]
fn test_memfs_create_dir_all_sync() {
    let fs = MemFs::new();
    let path = Path::new("/a/b/c/d");

    fs.create_dir_all_sync(path).unwrap();

    assert!(fs.exists_sync(path));
    assert!(fs.exists_sync(Path::new("/a/b/c")));
    assert!(fs.exists_sync(Path::new("/a/b")));
    assert!(fs.exists_sync(Path::new("/a")));
}

#[test]
fn test_memfs_metadata_sync() {
    let fs = MemFs::new();
    let path = Path::new("/tmp/test.txt");

    fs.write_sync(path, b"hello").unwrap();
    let meta = fs.metadata_sync(path).unwrap();

    assert_eq!(meta.len, 5);
    assert!(meta.is_file);
    assert!(!meta.is_dir);
}

#[test]
fn test_memfs_metadata_sync_dir() {
    let fs = MemFs::new();
    let path = Path::new("/tmp/mydir");

    fs.create_dir_all_sync(path).unwrap();
    let meta = fs.metadata_sync(path).unwrap();

    assert!(meta.is_dir);
    assert!(!meta.is_file);
}

#[test]
fn test_memfs_metadata_sync_not_found() {
    let fs = MemFs::new();
    let path = Path::new("/nonexistent");

    let result = fs.metadata_sync(path);
    assert!(result.is_err());
}

#[test]
fn test_memfs_remove_file_sync() {
    let fs = MemFs::new();
    let path = Path::new("/tmp/test.txt");

    fs.write_sync(path, b"content").unwrap();
    assert!(fs.exists_sync(path));

    fs.remove_file_sync(path).unwrap();
    assert!(!fs.exists_sync(path));
}

#[test]
fn test_memfs_copy_sync() {
    let fs = MemFs::new();
    let src = Path::new("/tmp/src.txt");
    let dst = Path::new("/tmp/dst.txt");

    fs.write_sync(src, b"copy me").unwrap();
    let bytes = fs.copy_sync(src, dst).unwrap();

    assert_eq!(bytes, 7);
    assert!(fs.exists_sync(dst));
}

#[test]
fn test_memfs_copy_sync_not_found() {
    let fs = MemFs::new();
    let src = Path::new("/nonexistent");
    let dst = Path::new("/tmp/dst.txt");

    let result = fs.copy_sync(src, dst);
    assert!(result.is_err());
}

#[tokio::test]
async fn test_memfs_symlink_sync() {
    let fs = MemFs::new();
    let original = Path::new("/tmp/original.txt");
    let link = Path::new("/tmp/link.txt");

    fs.write_sync(original, b"original content").unwrap();
    fs.symlink_sync(original, link).unwrap();

    // Verify symlink was created
    let target = fs.read_link(link).await.unwrap();
    assert_eq!(target, original);
}

#[test]
fn test_memfs_set_permissions_sync() {
    let fs = MemFs::new();
    let path = Path::new("/tmp/test.txt");

    fs.write_sync(path, b"content").unwrap();

    // set_permissions_sync is a no-op for MemFs, but should succeed
    fs.set_permissions_sync(path, 0o755).unwrap();
}

// Sync method tests for RealFs
#[test]
fn test_realfs_sync_methods() {
    let fs = RealFs;
    let temp_dir = tempfile::tempdir().unwrap();
    let path = temp_dir.path().join("test.txt");

    // Test write_sync
    fs.write_sync(&path, b"test content").unwrap();
    assert!(fs.exists_sync(&path));

    // Test metadata_sync
    let meta = fs.metadata_sync(&path).unwrap();
    assert!(meta.is_file);
    assert_eq!(meta.len, 12);

    // Test remove_file_sync
    fs.remove_file_sync(&path).unwrap();
    assert!(!fs.exists_sync(&path));
}

#[test]
fn test_realfs_create_dir_all_sync() {
    let fs = RealFs;
    let temp_dir = tempfile::tempdir().unwrap();
    let nested_dir = temp_dir.path().join("a/b/c");

    fs.create_dir_all_sync(&nested_dir).unwrap();
    assert!(fs.exists_sync(&nested_dir));
}

#[test]
fn test_realfs_copy_sync() {
    let fs = RealFs;
    let temp_dir = tempfile::tempdir().unwrap();
    let src = temp_dir.path().join("src.txt");
    let dst = temp_dir.path().join("dst.txt");

    fs.write_sync(&src, b"copy me").unwrap();
    let bytes = fs.copy_sync(&src, &dst).unwrap();

    assert_eq!(bytes, 7);
    assert!(fs.exists_sync(&dst));
}

#[cfg(unix)]
#[test]
fn test_realfs_symlink_sync() {
    let fs = RealFs;
    let temp_dir = tempfile::tempdir().unwrap();
    let original = temp_dir.path().join("original.txt");
    let link = temp_dir.path().join("link.txt");

    fs.write_sync(&original, b"original content").unwrap();
    fs.symlink_sync(&original, &link).unwrap();

    assert!(fs.exists_sync(&link));
}

#[cfg(unix)]
#[test]
fn test_realfs_set_permissions_sync() {
    let fs = RealFs;
    let temp_dir = tempfile::tempdir().unwrap();
    let path = temp_dir.path().join("test.txt");

    fs.write_sync(&path, b"content").unwrap();
    fs.set_permissions_sync(&path, 0o755).unwrap();

    // Verify permissions were set
    let meta = std::fs::metadata(&path).unwrap();
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(meta.permissions().mode() & 0o777, 0o755);
}

// =========================================================================
// RealFs/MemFs equivalence (design doc 2026-06-10, Phase 11)
//
// Each impl above has its own tests, which pin one backend's behavior in
// isolation - they cannot catch a case where the two disagree. These run
// the identical script against both, under the identical path strings (a
// real tempdir for RealFs; the same strings as pure in-memory keys for
// MemFs, which has no chroot), so any divergence shows up as a failing
// assertion naming exactly which step disagreed.
// =========================================================================

/// Runs the same directory/file/copy/remove script against `fs` and returns
/// each step's outcome as a plain, comparable value. Shape-checked (not
/// exact-content-checked past what the script itself writes) so it works
/// unmodified for both backends.
async fn run_core_equivalence_script(fs: &dyn FileSystem, root: &Path) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    let file = root.join("a.txt");
    let nested_dir = root.join("nested").join("dir");
    let nested_file = nested_dir.join("b.txt");
    let copy_dest = root.join("copy.txt");

    out.push((
        "create_dir_all",
        format!("{:?}", fs.create_dir_all(&nested_dir).await.is_ok()),
    ));
    out.push(("write", format!("{:?}", fs.write(&file, b"hello world").await.is_ok())));
    out.push((
        "write_nested",
        format!("{:?}", fs.write(&nested_file, b"nested content").await.is_ok()),
    ));
    out.push(("exists_file", fs.exists(&file).await.to_string()));
    out.push(("exists_missing", fs.exists(&root.join("nope.txt")).await.to_string()));
    out.push(("is_file_file", fs.is_file(&file).await.to_string()));
    out.push(("is_file_dir", fs.is_file(&nested_dir).await.to_string()));
    out.push(("is_dir_dir", fs.is_dir(&nested_dir).await.to_string()));
    out.push(("is_dir_file", fs.is_dir(&file).await.to_string()));
    out.push((
        "read_to_string",
        fs.read_to_string(&file).await.unwrap_or_else(|e| format!("ERR:{e}")),
    ));
    out.push((
        "read_to_string_missing",
        format!("{:?}", fs.read_to_string(&root.join("nope.txt")).await.is_err()),
    ));

    let meta_file = fs.metadata(&file).await.expect("file metadata");
    out.push(("metadata_file_is_file", meta_file.is_file.to_string()));
    out.push(("metadata_file_is_dir", meta_file.is_dir.to_string()));
    out.push(("metadata_file_len", meta_file.len.to_string()));

    let meta_dir = fs.metadata(&nested_dir).await.expect("dir metadata");
    out.push(("metadata_dir_is_file", meta_dir.is_file.to_string()));
    out.push(("metadata_dir_is_dir", meta_dir.is_dir.to_string()));

    out.push((
        "copy_len",
        fs.copy(&file, &copy_dest)
            .await
            .map(|n| n.to_string())
            .unwrap_or_else(|e| format!("ERR:{e}")),
    ));
    out.push(("copy_dest_exists", fs.exists(&copy_dest).await.to_string()));

    let mut children: Vec<String> = fs
        .read_dir(root)
        .await
        .expect("read_dir root")
        .into_iter()
        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
        .collect();
    children.sort();
    out.push(("read_dir_root", children.join(",")));

    out.push(("remove_file", format!("{:?}", fs.remove_file(&file).await.is_ok())));
    out.push(("exists_after_remove_file", fs.exists(&file).await.to_string()));

    out.push((
        "remove_dir_all",
        format!("{:?}", fs.remove_dir_all(&nested_dir).await.is_ok()),
    ));
    out.push(("exists_after_remove_dir", fs.exists(&nested_dir).await.to_string()));
    out.push((
        "exists_nested_file_after_remove_dir",
        fs.exists(&nested_file).await.to_string(),
    ));

    out
}

#[tokio::test]
async fn realfs_and_memfs_agree_on_the_core_async_script() {
    let temp_dir = tempfile::tempdir().unwrap();
    let real_root = temp_dir.path().to_path_buf();
    let real = RealFs;
    let real_out = run_core_equivalence_script(&real, &real_root).await;

    // MemFs has no chroot: any absolute-looking path works as a pure
    // in-memory key, so the identical root path is reused directly.
    let mem_root = PathBuf::from("/memfs-equivalence-root");
    let mem = MemFs::new();
    let mem_out = run_core_equivalence_script(&mem, &mem_root).await;

    assert_eq!(
        real_out, mem_out,
        "RealFs and MemFs must agree step by step on the same script"
    );
}

/// The one documented place the two backends deliberately disagree: `write`
/// to a path whose parent directory does not exist yet. `MemFs::write`
/// auto-creates the parent (`add_dir`); `RealFs::write` does not, matching
/// real `tokio::fs`/`std::fs` semantics. Every real caller in `src/` calls
/// `create_dir_all` first, so this never bites in practice - pinned here so
/// it stays a documented, deliberate difference rather than an untested one.
#[tokio::test]
async fn write_without_an_existing_parent_dir_is_the_one_documented_divergence() {
    let temp_dir = tempfile::tempdir().unwrap();
    let real_path = temp_dir.path().join("no-such-parent-dir").join("f.txt");
    let real = RealFs;
    assert!(
        real.write(&real_path, b"x").await.is_err(),
        "RealFs must not silently create missing parent directories"
    );

    let mem_path = PathBuf::from("/memfs-divergence-root/no-such-parent-dir/f.txt");
    let mem = MemFs::new();
    assert!(
        mem.write(&mem_path, b"x").await.is_ok(),
        "MemFs auto-creates the parent, unlike RealFs"
    );
}

/// The sync surface, same idea as the async script above but with the sync
/// methods, since they are a separate code path per implementation (not a
/// thin wrapper over the async ones).
fn run_core_equivalence_script_sync(fs: &dyn FileSystem, root: &Path) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    let file = root.join("a.txt");
    let copy_dest = root.join("copy.txt");

    out.push((
        "create_dir_all_sync",
        format!("{:?}", fs.create_dir_all_sync(root).is_ok()),
    ));
    out.push((
        "write_sync",
        format!("{:?}", fs.write_sync(&file, b"hello sync").is_ok()),
    ));
    out.push(("exists_sync_file", fs.exists_sync(&file).to_string()));
    out.push((
        "exists_sync_missing",
        fs.exists_sync(&root.join("nope.txt")).to_string(),
    ));
    out.push((
        "read_sync",
        fs.read_sync(&file)
            .map(|b| String::from_utf8_lossy(&b).to_string())
            .unwrap_or_else(|e| format!("ERR:{e}")),
    ));

    let meta = fs.metadata_sync(&file).expect("file metadata_sync");
    out.push(("metadata_sync_is_file", meta.is_file.to_string()));
    out.push(("metadata_sync_len", meta.len.to_string()));

    out.push((
        "copy_sync_len",
        fs.copy_sync(&file, &copy_dest)
            .map(|n| n.to_string())
            .unwrap_or_else(|e| format!("ERR:{e}")),
    ));
    out.push(("copy_sync_dest_exists", fs.exists_sync(&copy_dest).to_string()));

    out.push(("remove_file_sync", format!("{:?}", fs.remove_file_sync(&file).is_ok())));
    out.push(("exists_sync_after_remove", fs.exists_sync(&file).to_string()));

    out
}

#[test]
fn realfs_and_memfs_agree_on_the_core_sync_script() {
    let temp_dir = tempfile::tempdir().unwrap();
    let real = RealFs;
    let real_out = run_core_equivalence_script_sync(&real, temp_dir.path());

    let mem_root = PathBuf::from("/memfs-sync-equivalence-root");
    let mem = MemFs::new();
    let mem_out = run_core_equivalence_script_sync(&mem, &mem_root);

    assert_eq!(
        real_out, mem_out,
        "RealFs and MemFs must agree step by step on the same sync script"
    );
}

/// `read_link` agrees (both return the original target); `exists` does not,
/// and that is a second genuine divergence this audit found, not asserted
/// away. `RealFs::exists` is `tokio::fs::try_exists`, which follows a
/// symlink and reports whether its *target* exists. `MemFs::symlink` records
/// the link in a map entirely separate from `files`/`dirs`, and
/// `MemFs::exists` never consults it - so a `MemFs` symlink never "exists"
/// by this trait's own contract. No test in `src/` or `tests/` currently
/// calls `exists` on a path it knows to be a symlink, so nothing is
/// currently silently wrong in production, but a `MemFs`-backed unit test
/// that starts doing so would get a false negative. Recorded as a real gap
/// in `MemFs`'s fidelity, not fixed here (fixing it is a `MemFs` behavior
/// change, outside a test-writing phase).
#[cfg(unix)]
#[tokio::test]
async fn realfs_and_memfs_agree_on_read_link_but_diverge_on_exists_for_a_symlink() {
    let temp_dir = tempfile::tempdir().unwrap();
    let real_original = temp_dir.path().join("original.txt");
    let real_link = temp_dir.path().join("link.txt");
    let real = RealFs;
    real.write(&real_original, b"content").await.unwrap();
    real.symlink(&real_original, &real_link).await.unwrap();

    let mem_original = PathBuf::from("/memfs-symlink-root/original.txt");
    let mem_link = PathBuf::from("/memfs-symlink-root/link.txt");
    let mem = MemFs::new();
    mem.write(&mem_original, b"content").await.unwrap();
    mem.symlink(&mem_original, &mem_link).await.unwrap();

    assert_eq!(
        real.read_link(&real_link).await.unwrap(),
        real_original,
        "RealFs read_link must return the original target"
    );
    assert_eq!(
        mem.read_link(&mem_link).await.unwrap(),
        mem_original,
        "MemFs read_link must return the original target"
    );

    assert!(
        real.exists(&real_link).await,
        "RealFs::exists follows the symlink to its (existing) target"
    );
    assert!(
        !mem.exists(&mem_link).await,
        "MemFs::exists does not consult the symlinks map at all - documented divergence, not fixed here"
    );
}
