use std::time::Duration;

use arc_context::{
    Breadcrumb, OneLine, SessionTime, Vault, latest_handoff_at, resolve_dir, story_note_for,
    write_breadcrumb,
};
use arc_notes::BranchName;
use chrono::{DateTime, FixedOffset};

use crate::HookEnv;
use crate::activity::{self, SessionWork};
use crate::input::SessionEndInput;
use crate::marker::{Markers, SessionMarker};

const NOTE_LOCK_PATIENCE: Duration = Duration::from_millis(400);
const INDEX_BUSY_TIMEOUT: Duration = Duration::from_millis(300);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EndFacts {
    pub at: SessionTime,
    pub branch: Option<BranchName>,
    pub started_at: DateTime<FixedOffset>,
    pub last_handoff: Option<SessionTime>,
    pub work: Option<SessionWork>,
    pub uncommitted: Option<OneLine>,
}

pub fn breadcrumb(facts: EndFacts) -> Option<Breadcrumb> {
    let start = SessionTime::new(facts.started_at);
    let baseline = facts
        .last_handoff
        .map_or(start, |handoff| handoff.max(start));
    let work = facts.work?.since(baseline);
    if work.is_empty() {
        return None;
    }
    Breadcrumb::new(facts.at, facts.branch, work.subjects(), facts.uncommitted)
}

fn append(
    input: &SessionEndInput,
    marker: &SessionMarker,
    env: &HookEnv,
) -> anyhow::Result<Option<Breadcrumb>> {
    let Some(baseline) = &marker.baseline else {
        return Ok(None);
    };
    let dir = input.cwd.path();
    let Some(resolved) = resolve_dir(dir, &env.config)? else {
        return Ok(None);
    };
    let Some((rel, body)) = story_note_for(&resolved, &env.config)? else {
        return Ok(None);
    };
    let facts = EndFacts {
        at: SessionTime::new(env.now),
        branch: Some(resolved.branch),
        started_at: marker.started_at,
        last_handoff: latest_handoff_at(&body),
        work: activity::work_since(dir, baseline, env.now)?,
        uncommitted: activity::uncommitted(dir)?,
    };
    let Some(crumb) = breadcrumb(facts) else {
        return Ok(None);
    };
    let conn = arc_index::open_waiting(&env.config.cache_dir, INDEX_BUSY_TIMEOUT)?;
    let vault =
        Vault::new(&conn, &env.config.notes_dir, &env.config.cache_dir).within(NOTE_LOCK_PATIENCE);
    write_breadcrumb(&vault, &rel, &crumb)?;
    Ok(Some(crumb))
}

pub fn run(input: SessionEndInput, env: &HookEnv) -> anyhow::Result<Option<Breadcrumb>> {
    let markers = Markers::in_cache(&env.config.cache_dir);
    let Some(marker) = markers.load(&input.session_id)? else {
        return Ok(None);
    };
    let appended = append(&input, &marker, env);
    markers.remove(&input.session_id)?;
    appended
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::Commit;

    fn t(s: &str) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(s).unwrap()
    }

    fn line(s: &str) -> OneLine {
        OneLine::parse(s).unwrap()
    }

    fn commit(at: &str, subject: &str) -> Commit {
        Commit {
            committer: "me@x".into(),
            at: t(at),
            subject: line(subject),
        }
    }

    fn work(commits: Vec<Commit>, changes: &[&str]) -> Option<SessionWork> {
        Some(SessionWork {
            commits,
            changes: changes.iter().map(|at| t(at)).collect(),
        })
    }

    fn facts() -> EndFacts {
        EndFacts {
            at: SessionTime::new(t("2026-10-05T17:45:20+10:00")),
            branch: Some(BranchName::parse("me/sc-7/x").unwrap()),
            started_at: t("2026-10-05T09:00:30+10:00"),
            last_handoff: None,
            work: work(vec![], &[]),
            uncommitted: Some(line("1 file changed, 2 insertions(+)")),
        }
    }

    #[test]
    fn breadcrumbs_need_unlogged_session_work() {
        assert_eq!(breadcrumb(facts()), None);
        let legacy_marker = EndFacts {
            work: None,
            ..facts()
        };
        assert_eq!(breadcrumb(legacy_marker), None);
        let logged = EndFacts {
            last_handoff: Some(SessionTime::new(t("2026-10-05T12:00:00+10:00"))),
            work: work(
                vec![commit("2026-10-05T11:00:00+10:00", "x")],
                &["2026-10-05T12:00:40+10:00"],
            ),
            ..facts()
        };
        assert_eq!(breadcrumb(logged), None);
        let stale_handoff = EndFacts {
            last_handoff: Some(SessionTime::new(t("2026-10-04T09:00:00+10:00"))),
            work: work(vec![commit("2026-10-05T11:00:00+10:00", "x")], &[]),
            ..facts()
        };
        assert!(breadcrumb(stale_handoff).is_some());
    }

    #[test]
    fn work_after_an_in_session_handoff_is_recorded() {
        let after = EndFacts {
            last_handoff: Some(SessionTime::new(t("2026-10-05T12:00:00+10:00"))),
            work: work(
                vec![
                    commit("2026-10-05T13:00:00+10:00", "feat: after"),
                    commit("2026-10-05T11:00:00+10:00", "feat: logged"),
                ],
                &[],
            ),
            uncommitted: None,
            ..facts()
        };
        assert_eq!(
            breadcrumb(after).unwrap().to_string(),
            "### 2026-10-05 17:45 +10:00 · me/sc-7/x · [auto]\n**Commits:**\n- feat: after"
        );
    }

    #[test]
    fn golden_breadcrumbs() {
        let commits = EndFacts {
            work: work(
                vec![
                    commit("2026-10-05T10:00:00+10:00", "feat: flow"),
                    commit("2026-10-05T09:30:00+10:00", "fix: edge"),
                ],
                &[],
            ),
            ..facts()
        };
        assert_eq!(
            breadcrumb(commits).unwrap().to_string(),
            "### 2026-10-05 17:45 +10:00 · me/sc-7/x · [auto]\n**Commits:**\n- feat: flow\n- fix: edge\n**Uncommitted:** 1 file changed, 2 insertions(+)"
        );
        let edits = EndFacts {
            work: work(vec![], &["2026-10-05T12:00:00+10:00"]),
            ..facts()
        };
        assert_eq!(
            breadcrumb(edits).unwrap().to_string(),
            "### 2026-10-05 17:45 +10:00 · me/sc-7/x · [auto]\n**Uncommitted:** 1 file changed, 2 insertions(+)"
        );
    }
}
