use std::fs::File;
use std::path::Path;

use anyhow::{Context, Result, bail};
use fs2::FileExt;

const LOCK_FILE: &str = "daemon.lock";

pub struct DaemonLock {
    _held: File,
}

impl DaemonLock {
    pub fn acquire(cache_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(cache_dir)
            .with_context(|| format!("creating cache dir {}", cache_dir.display()))?;
        let path = cache_dir.join(LOCK_FILE);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .with_context(|| format!("opening {}", path.display()))?;
        if FileExt::try_lock_exclusive(&file).is_err() {
            bail!("another arc daemon is running (lock held on {})", path.display());
        }
        Ok(Self { _held: file })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_daemon_is_refused_while_first_holds_the_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let first = DaemonLock::acquire(tmp.path()).unwrap();

        let err = DaemonLock::acquire(tmp.path()).err().expect("lock is held");
        assert!(err.to_string().contains("another arc daemon is running"), "{err}");

        drop(first);
        assert!(DaemonLock::acquire(tmp.path()).is_ok());
    }
}
