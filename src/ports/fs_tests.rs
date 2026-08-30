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
