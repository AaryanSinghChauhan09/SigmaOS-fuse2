// SPDX-License-Identifier: MIT
//! SigmaOS-fuse2 — userspace virtual filesystem namespace and mount table.
//!
//! Provides path normalization, mount/umount records, and file operations over a
//! backing directory. Deterministic, dependency-free, and safe: paths cannot
//! escape the configured root.

#![forbid(unsafe_code)]
#![deny(clippy::all)]

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

/// Filesystem operation errors.
#[derive(Debug)]
pub enum FsError {
    InvalidPath(String),
    NotFound(String),
    AlreadyExists(String),
    NotADirectory(String),
    IsADirectory(String),
    MountExists(String),
    NotMounted(String),
    Io(io::Error),
}

impl fmt::Display for FsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPath(p) => write!(f, "invalid path: {p}"),
            Self::NotFound(p) => write!(f, "not found: {p}"),
            Self::AlreadyExists(p) => write!(f, "already exists: {p}"),
            Self::NotADirectory(p) => write!(f, "not a directory: {p}"),
            Self::IsADirectory(p) => write!(f, "is a directory: {p}"),
            Self::MountExists(p) => write!(f, "mount exists: {p}"),
            Self::NotMounted(p) => write!(f, "not mounted: {p}"),
            Self::Io(e) => write!(f, "io error: {e}"),
        }
    }
}

impl std::error::Error for FsError {}
impl From<io::Error> for FsError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// Metadata for a filesystem node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsStat {
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified_secs: u64,
}

/// A userspace filesystem namespace rooted at a backing directory.
pub struct Vfs {
    root: PathBuf,
    mounts: BTreeMap<String, String>,
}

impl Vfs {
    /// Open (or create) a VFS rooted at `root`. Mount table persists in
    /// `<root>/.sigmafs_mounts`.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, FsError> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(&root)?;
        let mount_file = root.join(".sigmafs_mounts");
        let mut mounts = BTreeMap::new();
        if mount_file.exists() {
            for line in fs::read_to_string(&mount_file)?.lines() {
                if let Some((target, source)) = line.split_once('\t') {
                    mounts.insert(target.to_string(), source.to_string());
                }
            }
        }
        Ok(Self { root, mounts })
    }

    fn mount_file(&self) -> PathBuf {
        self.root.join(".sigmafs_mounts")
    }

    fn save_mounts(&self) -> Result<(), FsError> {
        let mut out = String::new();
        for (target, source) in &self.mounts {
            out.push_str(&format!("{target}\t{source}\n"));
        }
        fs::write(self.mount_file(), out)?;
        Ok(())
    }

    /// Normalize a virtual path to a safe relative path inside the root.
    pub fn normalize(&self, path: &str) -> Result<PathBuf, FsError> {
        if path.contains('\0') {
            return Err(FsError::InvalidPath(path.to_string()));
        }
        let mut rel = PathBuf::new();
        for comp in Path::new(path).components() {
            match comp {
                Component::RootDir | Component::CurDir => {}
                Component::Normal(c) => rel.push(c),
                Component::ParentDir => {
                    if !rel.pop() {
                        return Err(FsError::InvalidPath(path.to_string()));
                    }
                }
                Component::Prefix(_) => return Err(FsError::InvalidPath(path.to_string())),
            }
        }
        Ok(rel)
    }

    /// Resolve a virtual path to a host path inside the root.
    pub fn resolve(&self, path: &str) -> Result<PathBuf, FsError> {
        Ok(self.root.join(self.normalize(path)?))
    }

    /// Mount `source` at virtual `target`.
    pub fn mount(&mut self, source: &str, target: &str) -> Result<(), FsError> {
        let t = self.normalize(target)?.to_string_lossy().to_string();
        if self.mounts.contains_key(&t) {
            return Err(FsError::MountExists(target.to_string()));
        }
        self.mounts.insert(t, source.to_string());
        self.save_mounts()
    }

    /// Unmount the mount at `target`.
    pub fn umount(&mut self, target: &str) -> Result<String, FsError> {
        let t = self.normalize(target)?.to_string_lossy().to_string();
        self.mounts.remove(&t).map(|s| {
            let _ = self.save_mounts();
            s
        }).ok_or_else(|| FsError::NotMounted(target.to_string()))
    }

    /// List active mounts as (target, source).
    pub fn mounts(&self) -> Vec<(String, String)> {
        self.mounts.iter().map(|(t, s)| (t.clone(), s.clone())).collect()
    }

    /// Create a directory (and parents) at `path`.
    pub fn mkdir(&self, path: &str) -> Result<(), FsError> {
        fs::create_dir_all(self.resolve(path)?)?;
        Ok(())
    }

    /// Write bytes to `path`, creating parent directories.
    pub fn write_file(&self, path: &str, data: &[u8]) -> Result<(), FsError> {
        let host = self.resolve(path)?;
        if let Some(parent) = host.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(host, data)?;
        Ok(())
    }

    /// Read bytes from `path`.
    pub fn read_file(&self, path: &str) -> Result<Vec<u8>, FsError> {
        let host = self.resolve(path)?;
        if !host.exists() {
            return Err(FsError::NotFound(path.to_string()));
        }
        if host.is_dir() {
            return Err(FsError::IsADirectory(path.to_string()));
        }
        Ok(fs::read(host)?)
    }

    /// List immediate children of `path`.
    pub fn list_dir(&self, path: &str) -> Result<Vec<String>, FsError> {
        let host = self.resolve(path)?;
        if !host.exists() {
            return Err(FsError::NotFound(path.to_string()));
        }
        if !host.is_dir() {
            return Err(FsError::NotADirectory(path.to_string()));
        }
        let mut names = Vec::new();
        for entry in fs::read_dir(host)? {
            let entry = entry?;
            names.push(entry.file_name().to_string_lossy().to_string());
        }
        names.sort();
        Ok(names)
    }

    /// Stat `path`.
    pub fn stat(&self, path: &str) -> Result<FsStat, FsError> {
        let host = self.resolve(path)?;
        if !host.exists() {
            return Err(FsError::NotFound(path.to_string()));
        }
        let meta = fs::metadata(&host)?;
        let modified_secs = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
        Ok(FsStat {
            path: path.to_string(),
            is_dir: meta.is_dir(),
            size: meta.len(),
            modified_secs,
        })
    }

    /// Remove a file at `path`.
    pub fn remove_file(&self, path: &str) -> Result<(), FsError> {
        let host = self.resolve(path)?;
        if !host.exists() {
            return Err(FsError::NotFound(path.to_string()));
        }
        if host.is_dir() {
            return Err(FsError::IsADirectory(path.to_string()));
        }
        fs::remove_file(host)?;
        Ok(())
    }

    /// Remove a directory at `path` (must be empty).
    pub fn remove_dir(&self, path: &str) -> Result<(), FsError> {
        let host = self.resolve(path)?;
        if !host.exists() {
            return Err(FsError::NotFound(path.to_string()));
        }
        if !host.is_dir() {
            return Err(FsError::NotADirectory(path.to_string()));
        }
        fs::remove_dir(host)?;
        Ok(())
    }

    /// Root backing path (for diagnostics).
    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_vfs(tag: &str) -> Vfs {
        let dir = std::env::temp_dir().join(format!("sigmaos-fuse2-{}-{}", std::process::id(), tag));
        let _ = fs::remove_dir_all(&dir);
        Vfs::open(&dir).unwrap()
    }

    #[test]
    fn test_normalize_rejects_escape() {
        let vfs = temp_vfs("norm");
        assert!(vfs.normalize("/a/../b").is_ok());
        assert!(vfs.normalize("/../../etc").is_err());
        assert!(vfs.normalize("/a//b/./c").unwrap() == PathBuf::from("a/b/c"));
    }

    #[test]
    fn test_write_read_list_stat() {
        let vfs = temp_vfs("rw");
        vfs.write_file("/docs/hello.txt", b"hello").unwrap();
        assert_eq!(vfs.read_file("/docs/hello.txt").unwrap(), b"hello");
        assert_eq!(vfs.list_dir("/docs").unwrap(), vec!["hello.txt".to_string()]);
        let st = vfs.stat("/docs/hello.txt").unwrap();
        assert!(!st.is_dir);
        assert_eq!(st.size, 5);
        assert!(vfs.stat("/docs").unwrap().is_dir);
    }

    #[test]
    fn test_mount_persist() {
        let vfs = temp_vfs("mnt");
        let mut v = vfs;
        v.mount("usb0", "/media/usb").unwrap();
        assert_eq!(v.mounts(), vec![("media/usb".to_string(), "usb0".to_string())]);
        let mut reopened = Vfs::open(v.root()).unwrap();
        assert_eq!(reopened.mounts().len(), 1);
        assert!(reopened.umount("/media/usb").is_ok());
        assert!(reopened.umount("/media/usb").is_err());
    }

    #[test]
    fn test_remove_and_errors() {
        let vfs = temp_vfs("rm");
        vfs.write_file("/f", b"x").unwrap();
        vfs.mkdir("/d").unwrap();
        assert!(matches!(vfs.read_file("/missing"), Err(FsError::NotFound(_))));
        assert!(matches!(vfs.read_file("/d"), Err(FsError::IsADirectory(_))));
        vfs.remove_file("/f").unwrap();
        vfs.remove_dir("/d").unwrap();
        assert!(matches!(vfs.stat("/f"), Err(FsError::NotFound(_))));
    }
}
