use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::NaiveDate;
use tracing::warn;

use crate::scheduler::ReminderKey;

const FIRED_FILE: &str = "fired-reminders";
const RETENTION: chrono::Duration = chrono::Duration::days(30);

pub(crate) fn is_remembered(reminder_date: NaiveDate, today: NaiveDate) -> bool {
    reminder_date >= today - RETENTION
}

pub struct FiredLog {
    path: PathBuf,
}

impl FiredLog {
    pub fn open(cache_dir: &Path, today: NaiveDate) -> Result<(Self, HashSet<ReminderKey>)> {
        let path = cache_dir.join(FIRED_FILE);
        let raw = match std::fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => {
                return Err(e).with_context(|| format!("reading fired reminders {}", path.display()));
            }
        };
        let complete_lines = raw.rsplit_once('\n').map_or("", |(complete, _torn)| complete);
        let parsed: Vec<_> = complete_lines
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| l.trim().parse::<ReminderKey>())
            .collect();
        let unparseable = parsed.iter().filter(|k| k.is_err()).count();
        if unparseable > 0 {
            warn!("dropping {unparseable} unparseable lines from {}", path.display());
        }
        let keys: HashSet<ReminderKey> = parsed
            .into_iter()
            .flatten()
            .filter(|k| is_remembered(k.date, today))
            .collect();

        let log = Self { path };
        log.rewrite(cache_dir, &keys)?;
        Ok((log, keys))
    }

    fn rewrite(&self, cache_dir: &Path, keys: &HashSet<ReminderKey>) -> Result<()> {
        std::fs::create_dir_all(cache_dir)
            .with_context(|| format!("creating cache dir {}", cache_dir.display()))?;
        let mut tmp = tempfile::NamedTempFile::new_in(cache_dir)?;
        tmp.write_all(lines(keys.iter()).as_bytes())?;
        tmp.as_file().sync_data()?;
        tmp.persist(&self.path)
            .with_context(|| format!("replacing {}", self.path.display()))?;
        Ok(())
    }

    pub fn record(&self, keys: &[ReminderKey]) -> std::io::Result<()> {
        if keys.is_empty() {
            return Ok(());
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        file.write_all(lines(keys.iter()).as_bytes())?;
        file.sync_data()
    }
}

fn lines<'a>(keys: impl Iterator<Item = &'a ReminderKey>) -> String {
    keys.map(|k| format!("{k}\n")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 6, 30).unwrap()
    }

    fn key(n: u128, date: NaiveDate) -> ReminderKey {
        ReminderKey {
            id: Uuid::from_u128(n),
            date,
        }
    }

    fn log_contents(dir: &Path) -> String {
        std::fs::read_to_string(dir.join(FIRED_FILE)).unwrap()
    }

    #[test]
    fn missing_log_loads_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let (_, keys) = FiredLog::open(tmp.path(), today()).unwrap();
        assert!(keys.is_empty());
    }

    #[test]
    fn recorded_keys_survive_reopening() {
        let tmp = tempfile::tempdir().unwrap();
        let a = key(1, today());
        let b = key(2, today());
        FiredLog::open(tmp.path(), today()).unwrap().0.record(&[a]).unwrap();
        FiredLog::open(tmp.path(), today()).unwrap().0.record(&[b]).unwrap();

        let (_, keys) = FiredLog::open(tmp.path(), today()).unwrap();
        assert_eq!(keys, HashSet::from([a, b]));
    }

    #[test]
    fn load_drops_legacy_lines_and_prunes_old_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let kept = key(1, today() - chrono::Duration::days(30));
        let pruned = key(2, today() - chrono::Duration::days(31));
        std::fs::write(
            tmp.path().join(FIRED_FILE),
            format!("{}\n{kept}\n{pruned}\n", Uuid::from_u128(3)),
        )
        .unwrap();

        let (_, keys) = FiredLog::open(tmp.path(), today()).unwrap();

        assert_eq!(keys, HashSet::from([kept]));
        assert_eq!(log_contents(tmp.path()), format!("{kept}\n"));
    }

    #[test]
    fn torn_last_line_is_ignored_and_next_append_starts_fresh() {
        let tmp = tempfile::tempdir().unwrap();
        let whole = key(1, today());
        let torn = key(2, today());
        let torn_line = &torn.to_string()[..20];
        std::fs::write(tmp.path().join(FIRED_FILE), format!("{whole}\n{torn_line}")).unwrap();

        let (log, keys) = FiredLog::open(tmp.path(), today()).unwrap();
        assert_eq!(keys, HashSet::from([whole]));
        log.record(&[torn]).unwrap();

        let (_, keys) = FiredLog::open(tmp.path(), today()).unwrap();
        assert_eq!(keys, HashSet::from([whole, torn]));
    }

    #[test]
    fn unreadable_log_fails_instead_of_loading_empty() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join(FIRED_FILE)).unwrap();

        assert!(FiredLog::open(tmp.path(), today()).is_err());
    }
}
