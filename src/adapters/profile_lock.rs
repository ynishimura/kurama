//! Per-profile lock that serializes token load, refresh and store across
//! kurama processes, so ten parallel `kurama api` calls refresh once.
//!
//! An advisory lock (`flock(2)` on Unix) on `<dir>/<profile>.lock`; the
//! file holds nothing.

use std::fs::{File, OpenOptions, TryLockError};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

pub struct ProfileLock {
    /// Held open for the guard's lifetime; closing it releases the lock.
    _file: File,
}

impl ProfileLock {
    /// Blocks until the lock is free; `on_wait` runs once when another
    /// process holds it.
    pub fn acquire(dir: &Path, profile: &str, on_wait: impl FnOnce()) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(lock_path(dir, profile))?;
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                on_wait();
                file.lock()?;
            }
            Err(TryLockError::Error(error)) => return Err(error),
        }
        Ok(Self { _file: file })
    }
}

/// `<dir>/<profile>.lock`, with anything but letters, digits, `.`, `_` and
/// `-` in the profile name replaced by `_`.
pub fn lock_path(dir: &Path, profile: &str) -> PathBuf {
    let name: String = profile
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    dir.join(format!("{name}.lock"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    #[test]
    fn the_lock_excludes_a_second_holder_until_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let held = ProfileLock::acquire(dir.path(), "github", || {}).unwrap();
        let other = File::open(lock_path(dir.path(), "github")).unwrap();
        assert!(matches!(other.try_lock(), Err(TryLockError::WouldBlock)));
        drop(held);
        other.try_lock().unwrap();
    }

    #[test]
    fn waiting_for_a_held_lock_is_announced_once() {
        let dir = tempfile::tempdir().unwrap();
        let held = ProfileLock::acquire(dir.path(), "github", || {}).unwrap();
        let announced = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&announced);
        let path = dir.path().to_path_buf();
        let waiter = std::thread::spawn(move || {
            ProfileLock::acquire(&path, "github", || flag.store(true, Ordering::SeqCst)).unwrap();
        });
        std::thread::sleep(Duration::from_millis(50));
        assert!(announced.load(Ordering::SeqCst));
        assert!(!waiter.is_finished());
        drop(held);
        waiter.join().unwrap();
        let mut waited = false;
        ProfileLock::acquire(dir.path(), "github", || waited = true).unwrap();
        assert!(!waited);
    }

    #[test]
    fn lock_names_are_file_safe() {
        assert_eq!(
            lock_path(Path::new("/locks"), "my/api name"),
            PathBuf::from("/locks/my_api_name.lock")
        );
    }
}
