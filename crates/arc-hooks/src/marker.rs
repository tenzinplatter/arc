use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::Context;
use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Serialize};

use crate::activity::Snapshot;
use crate::input::SessionId;

const SESSIONS_DIR: &str = "sessions";
pub const MARKER_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarkerState {
    Fresh,
    Nudged,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionMarker {
    pub session_id: SessionId,
    pub cwd: PathBuf,
    pub started_at: DateTime<FixedOffset>,
    pub state: MarkerState,
    #[serde(default)]
    pub baseline: Option<Snapshot>,
}

pub struct Markers(PathBuf);

impl Markers {
    pub fn in_cache(cache_dir: &Path) -> Self {
        Self(cache_dir.join(SESSIONS_DIR))
    }

    fn path(&self, id: &SessionId) -> PathBuf {
        self.0.join(format!("{id}.json"))
    }

    pub fn load(&self, id: &SessionId) -> anyhow::Result<Option<SessionMarker>> {
        let path = self.path(id);
        match std::fs::read_to_string(&path) {
            Ok(json) => Ok(Some(
                serde_json::from_str(&json).with_context(|| format!("parse {}", path.display()))?,
            )),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("read {}", path.display())),
        }
    }

    pub fn save(&self, marker: &SessionMarker) -> anyhow::Result<()> {
        std::fs::create_dir_all(&self.0)?;
        let mut tmp = tempfile::NamedTempFile::new_in(&self.0)?;
        tmp.write_all(&serde_json::to_vec_pretty(marker)?)?;
        tmp.persist(self.path(&marker.session_id))?;
        Ok(())
    }

    pub fn remove(&self, id: &SessionId) -> anyhow::Result<()> {
        match std::fs::remove_file(self.path(id)) {
            Err(e) if e.kind() != ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }

    pub fn prune(&self, now: SystemTime) -> anyhow::Result<usize> {
        let entries = match std::fs::read_dir(&self.0) {
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(0),
            entries => entries?,
        };
        let mut pruned = 0;
        for entry in entries.flatten() {
            let stale = entry
                .metadata()
                .and_then(|m| m.modified())
                .is_ok_and(|at| now.duration_since(at).is_ok_and(|age| age > MARKER_MAX_AGE));
            let is_marker = entry.path().extension().is_some_and(|ext| ext == "json");
            if stale && is_marker && std::fs::remove_file(entry.path()).is_ok() {
                pruned += 1;
            }
        }
        Ok(pruned)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::activity::{Change, DirtyEntry, Sha, StatusCode};

    fn marker(id: &str) -> SessionMarker {
        SessionMarker {
            session_id: SessionId::parse(id).unwrap(),
            cwd: "/work/repo".into(),
            started_at: DateTime::parse_from_rfc3339("2026-10-05T14:32:10+10:00").unwrap(),
            state: MarkerState::Fresh,
            baseline: Some(Snapshot {
                root: "/work/repo".into(),
                head: Some(Sha::parse(&"ab".repeat(20)).unwrap()),
                dirty: vec![DirtyEntry {
                    status: StatusCode::parse(" M").unwrap(),
                    path: "a.rs".into(),
                    change: Change::Modified(
                        DateTime::parse_from_rfc3339("2026-10-05T14:00:00+10:00").unwrap(),
                    ),
                }],
            }),
        }
    }

    #[test]
    fn markers_round_trip_and_remove() {
        let tmp = tempfile::tempdir().unwrap();
        let markers = Markers::in_cache(tmp.path());
        let m = marker("abc");
        assert_eq!(markers.load(&m.session_id).unwrap(), None);
        markers.save(&m).unwrap();
        assert_eq!(markers.load(&m.session_id).unwrap(), Some(m.clone()));
        let on_disk: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(tmp.path().join("sessions/abc.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            on_disk,
            json!({
                "session_id": "abc",
                "cwd": "/work/repo",
                "started_at": "2026-10-05T14:32:10+10:00",
                "state": "fresh",
                "baseline": {
                    "root": "/work/repo",
                    "head": "ab".repeat(20),
                    "dirty": [{"status": " M", "path": "a.rs", "change": {"modified": "2026-10-05T14:00:00+10:00"}}]
                }
            })
        );
        markers.remove(&m.session_id).unwrap();
        markers.remove(&m.session_id).unwrap();
        assert_eq!(markers.load(&m.session_id).unwrap(), None);
    }

    #[test]
    fn markers_from_before_baselines_still_load() {
        let tmp = tempfile::tempdir().unwrap();
        let markers = Markers::in_cache(tmp.path());
        std::fs::create_dir_all(tmp.path().join("sessions")).unwrap();
        std::fs::write(
            tmp.path().join("sessions/old.json"),
            r#"{"session_id": "old", "story_id": 7, "cwd": "/work/repo", "branch": "me/sc-7/x", "started_at": "2026-10-05T14:32:10+10:00", "state": "nudged"}"#,
        )
        .unwrap();
        let old = markers
            .load(&SessionId::parse("old").unwrap())
            .unwrap()
            .unwrap();
        assert_eq!((old.state, old.baseline), (MarkerState::Nudged, None));
    }

    #[test]
    fn prune_drops_only_old_markers() {
        let tmp = tempfile::tempdir().unwrap();
        let markers = Markers::in_cache(tmp.path());
        assert_eq!(markers.prune(SystemTime::now()).unwrap(), 0);
        markers.save(&marker("old")).unwrap();
        markers.save(&marker("new")).unwrap();
        std::fs::write(tmp.path().join("sessions/notes.txt"), "keep").unwrap();
        let old = std::fs::File::options()
            .write(true)
            .open(tmp.path().join("sessions/old.json"))
            .unwrap();
        old.set_modified(SystemTime::now() - MARKER_MAX_AGE - Duration::from_secs(60))
            .unwrap();
        assert_eq!(markers.prune(SystemTime::now()).unwrap(), 1);
        assert!(!tmp.path().join("sessions/old.json").exists());
        assert!(tmp.path().join("sessions/new.json").exists());
        assert!(tmp.path().join("sessions/notes.txt").exists());
    }
}
