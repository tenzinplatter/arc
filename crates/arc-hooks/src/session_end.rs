use arc_context::{
    Breadcrumb, OneLine, SessionTime, Vault, activity_since, latest_handoff_at, resolve_dir,
    story_note_for, write_breadcrumb,
};
use arc_notes::BranchName;
use chrono::{DateTime, FixedOffset};

use crate::HookEnv;
use crate::input::SessionEndInput;
use crate::marker::{Markers, SessionMarker};
use crate::repo::{self, Dirty};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EndFacts {
    pub at: SessionTime,
    pub branch: Option<BranchName>,
    pub started_at: DateTime<FixedOffset>,
    pub last_handoff: Option<SessionTime>,
    pub subjects: Vec<OneLine>,
    pub dirty: Option<Dirty>,
}

pub fn breadcrumb(facts: EndFacts) -> Option<Breadcrumb> {
    let start = SessionTime::new(facts.started_at);
    if facts.last_handoff.is_some_and(|handoff| handoff >= start) {
        return None;
    }
    let touched = facts
        .dirty
        .as_ref()
        .is_some_and(|d| activity_since(start, d.newest_change));
    if facts.subjects.is_empty() && !touched {
        return None;
    }
    Breadcrumb::new(
        facts.at,
        facts.branch,
        facts.subjects,
        facts.dirty.map(|d| d.summary),
    )
}

fn append(
    input: &SessionEndInput,
    marker: &SessionMarker,
    env: &HookEnv,
) -> anyhow::Result<Option<Breadcrumb>> {
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
        subjects: repo::commit_subjects_since(dir, marker.started_at)?,
        dirty: repo::activity(dir, env.now)?.dirty,
    };
    let Some(crumb) = breadcrumb(facts) else {
        return Ok(None);
    };
    let conn = arc_index::open(&env.config.cache_dir)?;
    let vault = Vault::new(&conn, &env.config.notes_dir, &env.config.cache_dir);
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

    fn t(s: &str) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(s).unwrap()
    }

    fn line(s: &str) -> OneLine {
        OneLine::parse(s).unwrap()
    }

    fn dirty(at: &str) -> Option<Dirty> {
        Some(Dirty {
            newest_change: t(at),
            summary: line("1 file changed, 2 insertions(+)"),
        })
    }

    fn facts() -> EndFacts {
        EndFacts {
            at: SessionTime::new(t("2026-10-05T17:45:20+10:00")),
            branch: Some(BranchName::parse("me/sc-7/x").unwrap()),
            started_at: t("2026-10-05T09:00:30+10:00"),
            last_handoff: None,
            subjects: vec![],
            dirty: None,
        }
    }

    #[test]
    fn breadcrumbs_need_unlogged_activity() {
        assert_eq!(breadcrumb(facts()), None);
        let old_dirt = EndFacts {
            dirty: dirty("2026-10-04T12:00:00+10:00"),
            ..facts()
        };
        assert_eq!(breadcrumb(old_dirt), None);
        let logged = EndFacts {
            last_handoff: Some(SessionTime::new(t("2026-10-05T09:00:00+10:00"))),
            subjects: vec![line("x")],
            ..facts()
        };
        assert_eq!(breadcrumb(logged), None);
        let stale_handoff = EndFacts {
            last_handoff: Some(SessionTime::new(t("2026-10-04T09:00:00+10:00"))),
            subjects: vec![line("x")],
            ..facts()
        };
        assert!(breadcrumb(stale_handoff).is_some());
    }

    #[test]
    fn golden_breadcrumbs() {
        let commits = EndFacts {
            subjects: vec![line("feat: flow"), line("fix: edge")],
            dirty: dirty("2026-10-04T12:00:00+10:00"),
            ..facts()
        };
        assert_eq!(
            breadcrumb(commits).unwrap().to_string(),
            "### 2026-10-05 17:45 +10:00 · me/sc-7/x · [auto]\n**Commits:**\n- feat: flow\n- fix: edge\n**Uncommitted:** 1 file changed, 2 insertions(+)"
        );
        let edits = EndFacts {
            dirty: dirty("2026-10-05T12:00:00+10:00"),
            ..facts()
        };
        assert_eq!(
            breadcrumb(edits).unwrap().to_string(),
            "### 2026-10-05 17:45 +10:00 · me/sc-7/x · [auto]\n**Uncommitted:** 1 file changed, 2 insertions(+)"
        );
    }
}
