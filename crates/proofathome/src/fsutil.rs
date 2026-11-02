//! Small file helpers: atomic writes, the state lock and random names.

use std::collections::hash_map::RandomState;
use std::fs::{self, OpenOptions};
use std::hash::{BuildHasher, Hasher};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use crate::error::{Error, Result};

/// A lock older than this belongs to a process that died while holding it.
/// Nothing holds the lock for longer than a few small file writes.
const STALE_LOCK: Duration = Duration::from_secs(30);
const LOCK_WAIT: Duration = Duration::from_secs(15);
const LOCK_RETRY: Duration = Duration::from_millis(20);

/// 16 random hex digits, from the standard library's randomly seeded hasher.
pub fn random_hex() -> String {
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u128(
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
    );
    hasher.write_u32(std::process::id());
    format!("{:016x}", hasher.finish())
}

pub fn create_dir_all(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(|e| Error::io("create directory", path, e))
}

/// Writes `bytes` to a temporary file next to `path` and renames it over
/// `path`, so readers see the old or the new content, never a mix.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::new(format!("{} has no parent directory", path.display())))?;
    create_dir_all(parent)?;
    let name = path
        .file_name()
        .map_or_else(|| "file".into(), |n| n.to_string_lossy());
    let temp = parent.join(format!(".{name}.tmp-{}", random_hex()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, path)
    })();
    result.map_err(|e| {
        let _ = fs::remove_file(&temp);
        Error::io("write", path, e)
    })
}

/// An exclusive lock on the state directory, held while reading and
/// updating state files. Released on drop.
#[derive(Debug)]
pub struct StateLock {
    path: PathBuf,
}

impl StateLock {
    pub fn acquire(dir: &Path) -> Result<StateLock> {
        create_dir_all(dir)?;
        let path = dir.join("lock");
        let start = Instant::now();
        loop {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    let _ = writeln!(file, "{}", std::process::id());
                    return Ok(StateLock { path });
                }
                Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                    if is_stale(&path) {
                        let _ = fs::remove_file(&path);
                        continue;
                    }
                }
                // Windows reports a file that is being deleted as access denied.
                Err(e) if e.kind() == ErrorKind::PermissionDenied && cfg!(windows) => {}
                Err(e) => return Err(Error::io("create lock file", &path, e)),
            }
            if start.elapsed() > LOCK_WAIT {
                return Err(Error::new(format!(
                    "the state is locked by another proofathome process; \
                     if none is running, remove {}",
                    path.display()
                )));
            }
            thread::sleep(LOCK_RETRY);
        }
    }
}

/// Whether a file was last modified longer than `STALE_LOCK` ago.
pub(crate) fn is_older_than(path: &Path, age: Duration) -> bool {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|elapsed| elapsed > age)
}

fn is_stale(path: &Path) -> bool {
    is_older_than(path, STALE_LOCK)
}

impl Drop for StateLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pah-fsutil-{name}-{}", random_hex()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn lock_is_exclusive() {
        let dir = Arc::new(temp_dir("lock"));
        let inside = Arc::new(AtomicU32::new(0));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let dir = Arc::clone(&dir);
                let inside = Arc::clone(&inside);
                thread::spawn(move || {
                    for _ in 0..10 {
                        let _lock = StateLock::acquire(&dir).unwrap();
                        assert_eq!(inside.fetch_add(1, Ordering::SeqCst), 0);
                        thread::sleep(Duration::from_millis(1));
                        inside.fetch_sub(1, Ordering::SeqCst);
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        assert!(!dir.join("lock").exists());
        fs::remove_dir_all(&*dir).unwrap();
    }

    #[test]
    fn atomic_write_replaces_content() {
        let dir = temp_dir("write");
        let path = dir.join("state.json");
        atomic_write(&path, b"one").unwrap();
        atomic_write(&path, b"two").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"two");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(&dir).unwrap();
    }
}
