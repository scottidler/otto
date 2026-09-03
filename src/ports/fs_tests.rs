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
async fn test_realfs_temp_file() {
    let fs = RealFs;
    let temp_dir = tempfile::tempdir().unwrap();
    let path = temp_dir.path().join("test.txt");

    fs.write(&path, b"test content").await.unwrap();
    assert!(fs.exists(&path).await);

    let content = fs.read_to_string(&path).await.unwrap();
    assert_eq!(content, "test content");
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

#[test]
fn test_memfs_symlink_sync() {
    let fs = MemFs::new();
    let original = Path::new("/tmp/original.txt");
    let link = Path::new("/tmp/link.txt");

    fs.write_sync(original, b"original content").unwrap();

    // MemFs has no async `read_link` to read the symlink back through the
    // trait; this only proves the write side succeeds.
    fs.symlink_sync(original, link).unwrap();
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

/// Runs the same directory/file script against `fs` and returns each step's
/// outcome as a plain, comparable value. Shape-checked (not exact-content-
/// checked past what the script itself writes) so it works unmodified for
/// both backends.
async fn run_core_equivalence_script(fs: &dyn FileSystem, root: &Path) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    let file = root.join("a.txt");
    let nested_dir = root.join("nested").join("dir");
    let nested_file = nested_dir.join("b.txt");

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
