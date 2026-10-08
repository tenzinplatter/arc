use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use uuid::Uuid;

const FIRED_FILE: &str = "fired-reminders";

pub struct FiredLog {
    path: PathBuf,
}

impl FiredLog {
    pub fn in_cache_dir(cache_dir: &Path) -> Self {
        Self {
            path: cache_dir.join(FIRED_FILE),
        }
    }

    pub fn load(&self) -> HashSet<Uuid> {
        std::fs::read_to_string(&self.path)
            .map(|raw| raw.lines().filter_map(|l| Uuid::parse_str(l.trim()).ok()).collect())
            .unwrap_or_default()
    }

    pub fn record(&self, ids: &[Uuid]) -> std::io::Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let lines: String = ids.iter().map(|id| format!("{id}\n")).collect();
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?
            .write_all(lines.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_log_loads_empty() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(FiredLog::in_cache_dir(tmp.path()).load().is_empty());
    }

    #[test]
    fn recorded_ids_survive_reopening() {
        let tmp = tempfile::tempdir().unwrap();
        let a = Uuid::from_u128(1);
        let b = Uuid::from_u128(2);
        FiredLog::in_cache_dir(tmp.path()).record(&[a]).unwrap();
        FiredLog::in_cache_dir(tmp.path()).record(&[b]).unwrap();

        assert_eq!(
            FiredLog::in_cache_dir(tmp.path()).load(),
            HashSet::from([a, b])
        );
    }
}
