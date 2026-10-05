use std::fmt;
use std::path::Path;
use std::time::SystemTime;

use arc_context::{StoryContext, current_context, resolve_dir};
use arc_core::dbg_file;
use arc_shortcut::Cache;

use crate::HookEnv;
use crate::activity::{self, Snapshot};
use crate::input::SessionStartInput;
use crate::marker::{MarkerState, Markers, SessionMarker};

pub struct Briefing<'a> {
    pub context: &'a StoryContext,
    pub cwd: &'a Path,
}

impl fmt::Display for Briefing<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let id = self.context.story.id;
        write!(f, "{}", self.context)?;
        writeln!(f, "\narc workflow for sc-{id}:")?;
        let canonical = match &self.context.canonical_branch {
            Some(branch) => format!(" (canonical: {branch})"),
            None => String::new(),
        };
        writeln!(
            f,
            "- New branches for this story: name them with `arc branch sc-{id} [--suffix <part>]`{canonical}; branches created from a story branch are linked to its story automatically when their name contains a `/` (e.g. `me/topic`)."
        )?;
        writeln!(
            f,
            "- Handoffs: call the arc MCP tool `log_session` with cwd: \"{}\" (Done / State / Next / Open questions) at milestones and before finishing.",
            self.cwd.display()
        )?;
        writeln!(
            f,
            "- Shortcut: create or update stories, workflow states and comments with the Shortcut MCP."
        )
    }
}

fn marker_for(
    input: &SessionStartInput,
    existing: Option<SessionMarker>,
    snapshot: Option<Snapshot>,
    env: &HookEnv,
) -> SessionMarker {
    let kept = existing.filter(|_| input.source.continues_session());
    SessionMarker {
        session_id: input.session_id.clone(),
        cwd: input.cwd.path().to_path_buf(),
        started_at: kept.as_ref().map_or(env.now, |m| m.started_at),
        state: kept.as_ref().map_or(MarkerState::Fresh, |m| m.state),
        baseline: kept.and_then(|m| m.baseline).or(snapshot),
    }
}

fn mark(input: &SessionStartInput, env: &HookEnv) -> anyhow::Result<()> {
    let markers = Markers::in_cache(&env.config.cache_dir);
    if let Err(e) = markers.prune(SystemTime::now()) {
        dbg_file!("arc hook session-start: pruning markers: {e:#}");
    }
    let existing = markers.load(&input.session_id).unwrap_or_else(|e| {
        dbg_file!("arc hook session-start: {e:#}");
        None
    });
    let snapshot = activity::snapshot(input.cwd.path(), env.now).unwrap_or_else(|e| {
        dbg_file!("arc hook session-start: snapshotting the repo: {e:#}");
        None
    });
    markers.save(&marker_for(input, existing, snapshot, env))
}

pub async fn run(
    input: SessionStartInput,
    env: &HookEnv,
    mut cache: Cache,
) -> anyhow::Result<Option<String>> {
    let dir = input.cwd.path();
    if let Err(e) = mark(&input, env) {
        dbg_file!("arc hook session-start: writing the session marker: {e:#}");
    }
    if resolve_dir(dir, &env.config)?.is_none() {
        return Ok(None);
    }
    let Some(context) = current_context(dir, &env.config, &mut cache).await? else {
        return Ok(None);
    };
    Ok(Some(
        Briefing {
            context: &context,
            cwd: dir,
        }
        .to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use arc_context::{ContextStory, ResolvedBy};
    use arc_notes::BranchName;
    use chrono::DateTime;

    use super::*;
    use crate::input::{Cwd, SessionId, StartSource};

    fn br(s: &str) -> BranchName {
        BranchName::parse(s).unwrap()
    }

    #[test]
    fn golden_briefing() {
        let context = StoryContext {
            story: ContextStory {
                id: 7.into(),
                details: None,
            },
            branch: br("me/sc-7/x"),
            resolved_by: ResolvedBy::BranchId,
            canonical_branch: Some(br("me/sc-7/story-seven")),
            note: None,
            sessions: vec![],
            open_todos: vec![],
        };
        assert_eq!(
            Briefing {
                context: &context,
                cwd: Path::new("/work/repo")
            }
            .to_string(),
            "Shortcut story sc-7 (details unavailable offline)\n\
             Branch: me/sc-7/x (story id in branch name)\n\
             Canonical branch: me/sc-7/story-seven\n\
             Note: none\n\
             \n\
             arc workflow for sc-7:\n\
             - New branches for this story: name them with `arc branch sc-7 [--suffix <part>]` (canonical: me/sc-7/story-seven); branches created from a story branch are linked to its story automatically when their name contains a `/` (e.g. `me/topic`).\n\
             - Handoffs: call the arc MCP tool `log_session` with cwd: \"/work/repo\" (Done / State / Next / Open questions) at milestones and before finishing.\n\
             - Shortcut: create or update stories, workflow states and comments with the Shortcut MCP.\n"
        );
    }

    fn env(now: &str) -> HookEnv {
        HookEnv {
            config: crate::tests::config(Path::new("/unused")),
            now: DateTime::parse_from_rfc3339(now).unwrap(),
            home: None,
        }
    }

    fn snapshot(root: &str) -> Option<Snapshot> {
        Some(Snapshot {
            root: root.into(),
            head: None,
            dirty: vec![],
        })
    }

    #[test]
    fn markers_keep_their_start_across_resume_and_compact() {
        let input = |source| SessionStartInput {
            session_id: SessionId::parse("s").unwrap(),
            cwd: Cwd::parse("/work/repo".into()).unwrap(),
            source,
        };
        let first = marker_for(
            &input(StartSource::Startup),
            None,
            snapshot("/first"),
            &env("2026-10-05T09:00:00+10:00"),
        );
        assert_eq!(
            (first.state, first.baseline.clone()),
            (MarkerState::Fresh, snapshot("/first"))
        );
        let nudged = SessionMarker {
            state: MarkerState::Nudged,
            ..first.clone()
        };
        let later = env("2026-10-05T11:00:00+10:00");
        for source in [StartSource::Resume, StartSource::Compact] {
            let kept = marker_for(
                &input(source),
                Some(nudged.clone()),
                snapshot("/later"),
                &later,
            );
            assert_eq!(
                (kept.started_at, kept.state, kept.baseline),
                (first.started_at, MarkerState::Nudged, snapshot("/first"))
            );
            let legacy = SessionMarker {
                baseline: None,
                ..nudged.clone()
            };
            let upgraded = marker_for(&input(source), Some(legacy), snapshot("/later"), &later);
            assert_eq!(upgraded.baseline, snapshot("/later"));
        }
        for source in [StartSource::Startup, StartSource::Clear, StartSource::Fork] {
            let fresh = marker_for(
                &input(source),
                Some(nudged.clone()),
                snapshot("/later"),
                &later,
            );
            assert_eq!(
                (fresh.started_at, fresh.state, fresh.baseline),
                (later.now, MarkerState::Fresh, snapshot("/later"))
            );
        }
    }
}
