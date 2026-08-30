use async_trait::async_trait;
use eyre::Result;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

/// Metadata about a file
#[derive(Debug, Clone)]
pub struct FileMetadata {
    pub len: u64,
    pub is_dir: bool,
    pub is_file: bool,
    pub is_symlink: bool,
}

/// Filesystem abstraction for dependency injection
#[async_trait]
pub trait FileSystem: Send + Sync {
    // Async methods
    async fn exists(&self, path: &Path) -> bool;
    async fn is_dir(&self, path: &Path) -> bool;
    async fn is_file(&self, path: &Path) -> bool;
    async fn metadata(&self, path: &Path) -> Result<FileMetadata>;
    async fn read_to_string(&self, path: &Path) -> Result<String>;
    async fn write(&self, path: &Path, contents: &[u8]) -> Result<()>;
    async fn create_dir_all(&self, path: &Path) -> Result<()>;
    async fn remove_file(&self, path: &Path) -> Result<()>;
    async fn remove_dir_all(&self, path: &Path) -> Result<()>;
    async fn copy(&self, from: &Path, to: &Path) -> Result<u64>;
    async fn read_dir(&self, path: &Path) -> Result<Vec<PathBuf>>;
    async fn read_link(&self, path: &Path) -> Result<PathBuf>;
    async fn symlink(&self, original: &Path, link: &Path) -> Result<()>;
    async fn set_permissions(&self, path: &Path, mode: u32) -> Result<()>;
    async fn canonicalize(&self, path: &Path) -> Result<PathBuf>;

    // Sync methods (for use in sync contexts like ActionProcessor)
    fn exists_sync(&self, path: &Path) -> bool;
    fn metadata_sync(&self, path: &Path) -> Result<FileMetadata>;
    fn read_sync(&self, path: &Path) -> Result<Vec<u8>>;
    /// Write `contents` to `path` atomically: a reader either sees the previous
    /// contents or the new ones, never a partial file.
    fn write_sync(&self, path: &Path, contents: &[u8]) -> Result<()>;
    fn create_dir_all_sync(&self, path: &Path) -> Result<()>;
    fn remove_file_sync(&self, path: &Path) -> Result<()>;
    fn copy_sync(&self, from: &Path, to: &Path) -> Result<u64>;
    fn symlink_sync(&self, original: &Path, link: &Path) -> Result<()>;
    fn set_permissions_sync(&self, path: &Path, mode: u32) -> Result<()>;
}

/// Real filesystem implementation using tokio::fs
#[derive(Debug, Clone, Default)]
pub struct RealFs;

/// Write `contents` to `path` via a temporary file in the same directory plus a
/// rename.
///
/// `fs::write` truncates first and then writes, so a crash (or a full disk) in
/// between leaves a torn file under the final name. That matters most for the
/// content-addressed script cache, where the name is a promise about the
/// contents: a truncated entry kept its name and was re-executed forever.
/// Same-directory staging keeps the rename on one filesystem, where it is atomic.
fn atomic_write(path: &Path, contents: &[u8]) -> Result<()> {
    use std::io::Write;

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(contents)?;
    temp.as_file().sync_all()?;
    temp.persist(path)
        .map_err(|e| eyre::eyre!("Failed to persist {}: {}", path.display(), e.error))?;
    Ok(())
}

#[async_trait]
impl FileSystem for RealFs {
    async fn exists(&self, path: &Path) -> bool {
        tokio::fs::try_exists(path).await.unwrap_or(false)
    }

    async fn is_dir(&self, path: &Path) -> bool {
        tokio::fs::metadata(path).await.map(|m| m.is_dir()).unwrap_or(false)
    }

    async fn is_file(&self, path: &Path) -> bool {
        tokio::fs::metadata(path).await.map(|m| m.is_file()).unwrap_or(false)
    }

    async fn metadata(&self, path: &Path) -> Result<FileMetadata> {
        let meta = tokio::fs::metadata(path).await?;
        Ok(FileMetadata {
            len: meta.len(),
            is_dir: meta.is_dir(),
            is_file: meta.is_file(),
            is_symlink: meta.is_symlink(),
        })
    }

    async fn read_to_string(&self, path: &Path) -> Result<String> {
        Ok(tokio::fs::read_to_string(path).await?)
    }

    async fn write(&self, path: &Path, contents: &[u8]) -> Result<()> {
        let path = path.to_path_buf();
        let contents = contents.to_vec();
        tokio::task::spawn_blocking(move || atomic_write(&path, &contents)).await?
    }

    async fn create_dir_all(&self, path: &Path) -> Result<()> {
        Ok(tokio::fs::create_dir_all(path).await?)
    }

    async fn remove_file(&self, path: &Path) -> Result<()> {
        Ok(tokio::fs::remove_file(path).await?)
    }

    async fn remove_dir_all(&self, path: &Path) -> Result<()> {
        Ok(tokio::fs::remove_dir_all(path).await?)
    }

    async fn copy(&self, from: &Path, to: &Path) -> Result<u64> {
        Ok(tokio::fs::copy(from, to).await?)
    }

    async fn read_dir(&self, path: &Path) -> Result<Vec<PathBuf>> {
        let mut entries = Vec::new();
        let mut dir = tokio::fs::read_dir(path).await?;
        while let Some(entry) = dir.next_entry().await? {
            entries.push(entry.path());
        }
        Ok(entries)
    }

    async fn read_link(&self, path: &Path) -> Result<PathBuf> {
        Ok(tokio::fs::read_link(path).await?)
    }

    async fn symlink(&self, original: &Path, link: &Path) -> Result<()> {
        #[cfg(unix)]
        {
            Ok(tokio::fs::symlink(original, link).await?)
        }
        #[cfg(not(unix))]
        {
            Err(eyre::eyre!("Symlinks not supported on this platform"))
        }
    }

    async fn set_permissions(&self, path: &Path, mode: u32) -> Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(mode);
            Ok(tokio::fs::set_permissions(path, perms).await?)
        }
        #[cfg(not(unix))]
        {
            let _ = (path, mode);
            Ok(()) // No-op on non-Unix
        }
    }

    async fn canonicalize(&self, path: &Path) -> Result<PathBuf> {
        Ok(tokio::fs::canonicalize(path).await?)
    }

    // Sync methods
    fn exists_sync(&self, path: &Path) -> bool {
        path.exists()
    }

    fn metadata_sync(&self, path: &Path) -> Result<FileMetadata> {
        let meta = std::fs::metadata(path)?;
        Ok(FileMetadata {
            len: meta.len(),
            is_dir: meta.is_dir(),
            is_file: meta.is_file(),
            is_symlink: meta.is_symlink(),
        })
    }

    fn read_sync(&self, path: &Path) -> Result<Vec<u8>> {
        Ok(std::fs::read(path)?)
    }

    fn write_sync(&self, path: &Path, contents: &[u8]) -> Result<()> {
        atomic_write(path, contents)
    }

    fn create_dir_all_sync(&self, path: &Path) -> Result<()> {
        Ok(std::fs::create_dir_all(path)?)
    }

    fn remove_file_sync(&self, path: &Path) -> Result<()> {
        Ok(std::fs::remove_file(path)?)
    }

    fn copy_sync(&self, from: &Path, to: &Path) -> Result<u64> {
        Ok(std::fs::copy(from, to)?)
    }

    fn symlink_sync(&self, original: &Path, link: &Path) -> Result<()> {
        #[cfg(unix)]
        {
            Ok(std::os::unix::fs::symlink(original, link)?)
        }
        #[cfg(not(unix))]
        {
            Err(eyre::eyre!("Symlinks not supported on this platform"))
        }
    }

    fn set_permissions_sync(&self, path: &Path, mode: u32) -> Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(mode);
            Ok(std::fs::set_permissions(path, perms)?)
        }
        #[cfg(not(unix))]
        {
            let _ = (path, mode);
            Ok(()) // No-op on non-Unix
        }
    }
}

/// In-memory filesystem for testing
#[derive(Debug, Clone, Default)]
pub struct MemFs {
    files: Arc<RwLock<HashMap<PathBuf, Vec<u8>>>>,
    dirs: Arc<RwLock<std::collections::HashSet<PathBuf>>>,
    symlinks: Arc<RwLock<HashMap<PathBuf, PathBuf>>>,
}

impl MemFs {
    pub fn new() -> Self {
        Self {
            files: Arc::new(RwLock::new(HashMap::new())),
            dirs: Arc::new(RwLock::new(std::collections::HashSet::new())),
            symlinks: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Add a file with content for testing
    pub fn add_file(&self, path: impl AsRef<Path>, content: impl AsRef<[u8]>) {
        let path = path.as_ref().to_path_buf();
        self.files
            .write()
            .unwrap()
            .insert(path.clone(), content.as_ref().to_vec());

        // Add parent directories
        if let Some(parent) = path.parent() {
            self.add_dir(parent);
        }
    }

    /// Add a directory for testing
    pub fn add_dir(&self, path: impl AsRef<Path>) {
        let path = path.as_ref().to_path_buf();
        self.dirs.write().unwrap().insert(path.clone());

        // Add parent directories recursively
        if let Some(parent) = path.parent()
            && parent != Path::new("")
        {
            self.add_dir(parent);
        }
    }
}

#[async_trait]
impl FileSystem for MemFs {
    async fn exists(&self, path: &Path) -> bool {
        let path = path.to_path_buf();
        self.files.read().unwrap().contains_key(&path) || self.dirs.read().unwrap().contains(&path)
    }

    async fn is_dir(&self, path: &Path) -> bool {
        self.dirs.read().unwrap().contains(&path.to_path_buf())
    }

    async fn is_file(&self, path: &Path) -> bool {
        self.files.read().unwrap().contains_key(&path.to_path_buf())
    }

    async fn metadata(&self, path: &Path) -> Result<FileMetadata> {
        let path = path.to_path_buf();
        let files = self.files.read().unwrap();
        let dirs = self.dirs.read().unwrap();

        if let Some(content) = files.get(&path) {
            Ok(FileMetadata {
                len: content.len() as u64,
                is_dir: false,
                is_file: true,
                is_symlink: false,
            })
        } else if dirs.contains(&path) {
            Ok(FileMetadata {
                len: 0,
                is_dir: true,
                is_file: false,
                is_symlink: false,
            })
        } else {
            Err(eyre::eyre!("Path not found: {}", path.display()))
        }
    }

    async fn read_to_string(&self, path: &Path) -> Result<String> {
        let files = self.files.read().unwrap();
        let content = files
            .get(&path.to_path_buf())
            .ok_or_else(|| eyre::eyre!("File not found: {}", path.display()))?;
        Ok(String::from_utf8_lossy(content).to_string())
    }

    async fn write(&self, path: &Path, contents: &[u8]) -> Result<()> {
        let path = path.to_path_buf();

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            self.add_dir(parent);
        }

        self.files.write().unwrap().insert(path, contents.to_vec());
        Ok(())
    }

    async fn create_dir_all(&self, path: &Path) -> Result<()> {
        self.add_dir(path);
        Ok(())
    }

    async fn remove_file(&self, path: &Path) -> Result<()> {
        self.files.write().unwrap().remove(&path.to_path_buf());
        Ok(())
    }

    async fn remove_dir_all(&self, path: &Path) -> Result<()> {
        let path = path.to_path_buf();
        let mut files = self.files.write().unwrap();
        let mut dirs = self.dirs.write().unwrap();

        // Remove all files under this path
        files.retain(|k, _| !k.starts_with(&path));
        // Remove all dirs under this path
        dirs.retain(|k| !k.starts_with(&path));

        Ok(())
    }

    async fn copy(&self, from: &Path, to: &Path) -> Result<u64> {
        let content = {
            let files = self.files.read().unwrap();
            files
                .get(&from.to_path_buf())
                .cloned()
                .ok_or_else(|| eyre::eyre!("Source file not found: {}", from.display()))?
        };

        let len = content.len() as u64;

        // Ensure parent directory exists
        if let Some(parent) = to.parent() {
            self.add_dir(parent);
        }

        self.files.write().unwrap().insert(to.to_path_buf(), content);
        Ok(len)
    }

    async fn read_dir(&self, path: &Path) -> Result<Vec<PathBuf>> {
        let path = path.to_path_buf();
        let files = self.files.read().unwrap();
        let dirs = self.dirs.read().unwrap();

        let mut entries = std::collections::HashSet::new();

        // Find all direct children (files)
        for file_path in files.keys() {
            if let Some(parent) = file_path.parent()
                && parent == path
            {
                entries.insert(file_path.clone());
            }
        }

        // Find all direct children (directories)
        for dir_path in dirs.iter() {
            if let Some(parent) = dir_path.parent()
                && parent == path
            {
                entries.insert(dir_path.clone());
            }
        }

        Ok(entries.into_iter().collect())
    }

    async fn read_link(&self, path: &Path) -> Result<PathBuf> {
        let symlinks = self.symlinks.read().unwrap();
        symlinks
            .get(&path.to_path_buf())
            .cloned()
            .ok_or_else(|| eyre::eyre!("Symlink not found: {}", path.display()))
    }

    async fn symlink(&self, original: &Path, link: &Path) -> Result<()> {
        // Ensure parent directory exists
        if let Some(parent) = link.parent() {
            self.add_dir(parent);
        }
        self.symlinks
            .write()
            .unwrap()
            .insert(link.to_path_buf(), original.to_path_buf());
        Ok(())
    }

    async fn set_permissions(&self, _path: &Path, _mode: u32) -> Result<()> {
        // No-op for in-memory filesystem - permissions aren't tracked
        Ok(())
    }

    async fn canonicalize(&self, path: &Path) -> Result<PathBuf> {
        // For MemFs, just return the path if it exists
        if self.exists(path).await {
            Ok(path.to_path_buf())
        } else {
            Err(eyre::eyre!("Path not found: {}", path.display()))
        }
    }

    // Sync methods
    fn exists_sync(&self, path: &Path) -> bool {
        let path = path.to_path_buf();
        self.files.read().unwrap().contains_key(&path) || self.dirs.read().unwrap().contains(&path)
    }

    fn metadata_sync(&self, path: &Path) -> Result<FileMetadata> {
        let path = path.to_path_buf();
        let files = self.files.read().unwrap();
        let dirs = self.dirs.read().unwrap();

        if let Some(content) = files.get(&path) {
            Ok(FileMetadata {
                len: content.len() as u64,
                is_dir: false,
                is_file: true,
                is_symlink: false,
            })
        } else if dirs.contains(&path) {
            Ok(FileMetadata {
                len: 0,
                is_dir: true,
                is_file: false,
                is_symlink: false,
            })
        } else {
            Err(eyre::eyre!("Path not found: {}", path.display()))
        }
    }

    fn read_sync(&self, path: &Path) -> Result<Vec<u8>> {
        let files = self.files.read().unwrap();
        files
            .get(&path.to_path_buf())
            .cloned()
            .ok_or_else(|| eyre::eyre!("File not found: {}", path.display()))
    }

    fn write_sync(&self, path: &Path, contents: &[u8]) -> Result<()> {
        let path = path.to_path_buf();

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            self.add_dir(parent);
        }

        // Inserting the whole value under the lock is already atomic for readers.
        self.files.write().unwrap().insert(path, contents.to_vec());
        Ok(())
    }

    fn create_dir_all_sync(&self, path: &Path) -> Result<()> {
        self.add_dir(path);
        Ok(())
    }

    fn remove_file_sync(&self, path: &Path) -> Result<()> {
        self.files.write().unwrap().remove(&path.to_path_buf());
        Ok(())
    }

    fn copy_sync(&self, from: &Path, to: &Path) -> Result<u64> {
        let content = {
            let files = self.files.read().unwrap();
            files
                .get(&from.to_path_buf())
                .cloned()
                .ok_or_else(|| eyre::eyre!("Source file not found: {}", from.display()))?
        };

        let len = content.len() as u64;

        // Ensure parent directory exists
        if let Some(parent) = to.parent() {
            self.add_dir(parent);
        }

        self.files.write().unwrap().insert(to.to_path_buf(), content);
        Ok(len)
    }

    fn symlink_sync(&self, original: &Path, link: &Path) -> Result<()> {
        // Ensure parent directory exists
        if let Some(parent) = link.parent() {
            self.add_dir(parent);
        }
        self.symlinks
            .write()
            .unwrap()
            .insert(link.to_path_buf(), original.to_path_buf());
        Ok(())
    }

    fn set_permissions_sync(&self, _path: &Path, _mode: u32) -> Result<()> {
        // No-op for in-memory filesystem - permissions aren't tracked
        Ok(())
    }
}

#[cfg(test)]
mod tests {
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
}
