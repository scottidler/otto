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
    /// Create exactly this directory, failing if it already exists.
    ///
    /// The failure is the point: it is the only way to reserve a directory name
    /// against other processes, because the check and the creation happen in one
    /// syscall. `create_dir_all` succeeds on an existing directory and so cannot
    /// tell a caller whether it won the name.
    async fn create_dir_exclusive(&self, path: &Path) -> Result<bool>;
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

    async fn create_dir_exclusive(&self, path: &Path) -> Result<bool> {
        match tokio::fs::create_dir(path).await {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
            Err(e) => Err(e.into()),
        }
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

    async fn create_dir_exclusive(&self, path: &Path) -> Result<bool> {
        if self.is_dir(path).await {
            return Ok(false);
        }
        self.add_dir(path);
        Ok(true)
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

#[path = "fs_tests.rs"]
mod tests;
