use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Context;
use arc_core::StoryId;
use fs2::FileExt;
use tempfile::NamedTempFile;

use crate::path::NoteRelPath;

const LOCKS_DIR: &str = "locks";
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0100_0000_01b3;
#[cfg(unix)]
const NEW_FILE_MODE: u32 = 0o666;

fn fnv1a(key: &str) -> u64 {
    key.bytes().fold(FNV_OFFSET, |hash, b| {
        (hash ^ u64::from(b)).wrapping_mul(FNV_PRIME)
    })
}

#[derive(Clone, Debug)]
pub struct NoteLocks(PathBuf);

#[derive(Debug)]
pub struct NoteLock(File);

impl Drop for NoteLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

impl NoteLocks {
    pub fn in_cache(cache_dir: &Path) -> Self {
        Self(cache_dir.join(LOCKS_DIR))
    }

    pub fn note(&self, rel: &NoteRelPath) -> anyhow::Result<NoteLock> {
        self.hold(&format!("note:{rel}"))
    }

    pub fn story(&self, id: StoryId) -> anyhow::Result<NoteLock> {
        self.hold(&format!("story:{id}"))
    }

    fn hold(&self, key: &str) -> anyhow::Result<NoteLock> {
        fs::create_dir_all(&self.0)
            .with_context(|| format!("create lock dir {}", self.0.display()))?;
        let path = self.0.join(format!("{:016x}.lock", fnv1a(key)));
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .with_context(|| format!("open lock {}", path.display()))?;
        FileExt::lock_exclusive(&file).with_context(|| format!("lock {}", path.display()))?;
        Ok(NoteLock(file))
    }
}

fn filled(mut tmp: NamedTempFile, content: &str) -> anyhow::Result<NamedTempFile> {
    tmp.write_all(content.as_bytes())?;
    tmp.as_file().sync_all()?;
    Ok(tmp)
}

pub fn replace(path: &Path, content: &str) -> anyhow::Result<()> {
    let target = fs::canonicalize(path).with_context(|| format!("resolve {}", path.display()))?;
    let dir = target
        .parent()
        .with_context(|| format!("{} has no parent directory", target.display()))?;
    let tmp = NamedTempFile::new_in(dir)?;
    tmp.as_file()
        .set_permissions(fs::metadata(&target)?.permissions())?;
    filled(tmp, content)?
        .persist(&target)
        .with_context(|| format!("replace {}", target.display()))?;
    Ok(())
}

pub fn create(path: &Path, content: &str) -> anyhow::Result<()> {
    let dir = path
        .parent()
        .with_context(|| format!("{} has no parent directory", path.display()))?;
    fs::create_dir_all(dir)?;
    let mut builder = tempfile::Builder::new();
    #[cfg(unix)]
    builder.permissions(std::os::unix::fs::PermissionsExt::from_mode(NEW_FILE_MODE));
    filled(builder.tempfile_in(dir)?, content)?
        .persist_noclobber(path)
        .with_context(|| format!("create {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};

    use super::*;

    fn rel(s: &str) -> NoteRelPath {
        NoteRelPath::parse(s).unwrap()
    }

    #[test]
    fn lock_files_live_in_the_cache_keyed_by_note() {
        let cache = tempfile::tempdir().unwrap();
        let locks = NoteLocks::in_cache(cache.path());
        let a = locks.note(&rel("stories/a.md")).unwrap();
        let b = locks.note(&rel("stories/b.md")).unwrap();
        let s = locks.story(7.into()).unwrap();
        drop((a, b, s));
        assert_eq!(
            fs::read_dir(cache.path().join(LOCKS_DIR)).unwrap().count(),
            3
        );
        assert_eq!(fnv1a("note:stories/a.md"), fnv1a("note:stories/a.md"));
        assert_eq!(fnv1a(""), FNV_OFFSET);
    }

    #[test]
    fn locked_read_modify_writes_do_not_lose_updates() {
        let cache = tempfile::tempdir().unwrap();
        let notes = tempfile::tempdir().unwrap();
        let path = notes.path().join("n.md");
        fs::write(&path, "").unwrap();
        let threads = 8;
        let barrier = Arc::new(Barrier::new(threads));
        let handles: Vec<_> = (0..threads)
            .map(|i| {
                let (locks, path, barrier) = (
                    NoteLocks::in_cache(cache.path()),
                    path.clone(),
                    barrier.clone(),
                );
                std::thread::spawn(move || {
                    barrier.wait();
                    let _lock = locks.note(&rel("n.md")).unwrap();
                    let body = fs::read_to_string(&path).unwrap();
                    replace(&path, &format!("{body}{i}\n")).unwrap();
                })
            })
            .collect();
        handles.into_iter().for_each(|h| h.join().unwrap());
        assert_eq!(fs::read_to_string(&path).unwrap().lines().count(), threads);
    }

    #[test]
    fn replace_keeps_permissions_and_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.md");
        fs::write(&real, "old").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&real, fs::Permissions::from_mode(0o640)).unwrap();
            let link = dir.path().join("link.md");
            std::os::unix::fs::symlink(&real, &link).unwrap();
            replace(&link, "new").unwrap();
            assert!(
                fs::symlink_metadata(&link)
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
            assert_eq!(
                fs::metadata(&real).unwrap().permissions().mode() & 0o777,
                0o640
            );
        }
        #[cfg(not(unix))]
        replace(&real, "new").unwrap();
        assert_eq!(fs::read_to_string(&real).unwrap(), "new");
        assert_eq!(
            fs::read_dir(dir.path()).unwrap().count(),
            1 + usize::from(cfg!(unix))
        );
    }

    #[test]
    fn create_never_clobbers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/new.md");
        create(&path, "a").unwrap();
        assert!(create(&path, "b").is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "a");
        assert_eq!(fs::read_dir(dir.path().join("sub")).unwrap().count(), 1);
    }
}
