use arc_context::{SessionTime, activity_since, latest_handoff_at, resolve_dir, story_note_for};
use chrono::{DateTime, FixedOffset};

use crate::HookEnv;
use crate::activity;
use crate::input::StopInput;
use crate::marker::{MarkerState, Markers};
use crate::output::BlockOutput;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionFacts {
    pub state: MarkerState,
    pub started_at: DateTime<FixedOffset>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StopFacts {
    pub session: Option<SessionFacts>,
    pub last_handoff: Option<SessionTime>,
    pub activity: Option<DateTime<FixedOffset>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopDecision {
    Silent,
    AskForHandoff,
}

pub fn decide(facts: StopFacts) -> StopDecision {
    let Some(SessionFacts {
        state: MarkerState::Fresh,
        started_at,
    }) = facts.session
    else {
        return StopDecision::Silent;
    };
    let start = SessionTime::new(started_at);
    let baseline = facts
        .last_handoff
        .map_or(start, |handoff| handoff.max(start));
    match facts.activity {
        Some(activity) if activity_since(baseline, activity) => StopDecision::AskForHandoff,
        Some(_) | None => StopDecision::Silent,
    }
}

fn reason(story: arc_core::StoryId, input: &StopInput) -> String {
    format!(
        "arc: there is work on sc-{story} since the last session handoff. Before finishing, call the arc MCP tool `log_session` with cwd: \"{}\" and Done / State / Next / Open questions so the next session can pick up from here. (Asked once per session.)",
        input.cwd.path().display()
    )
}

pub fn run(input: StopInput, env: &HookEnv) -> anyhow::Result<Option<String>> {
    let markers = Markers::in_cache(&env.config.cache_dir);
    let Some(mut marker) = markers.load(&input.session_id)? else {
        return Ok(None);
    };
    if marker.state == MarkerState::Nudged {
        return Ok(None);
    }
    let dir = input.cwd.path();
    let Some(resolved) = resolve_dir(dir, &env.config)? else {
        return Ok(None);
    };
    let last_handoff =
        story_note_for(&resolved, &env.config)?.and_then(|(_, body)| latest_handoff_at(&body));
    let work = match &marker.baseline {
        Some(baseline) => activity::work_since(dir, baseline, env.now)?,
        None => None,
    };
    let facts = StopFacts {
        session: Some(SessionFacts {
            state: marker.state,
            started_at: marker.started_at,
        }),
        last_handoff,
        activity: work.and_then(|w| w.latest()),
    };
    match decide(facts) {
        StopDecision::Silent => Ok(None),
        StopDecision::AskForHandoff => {
            marker.state = MarkerState::Nudged;
            markers.save(&marker)?;
            let block = BlockOutput::block(reason(resolved.id, &input));
            Ok(Some(serde_json::to_string(&block)?))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(s).unwrap()
    }

    fn entry(s: &str) -> SessionTime {
        SessionTime::new(t(s))
    }

    fn session(state: MarkerState) -> Option<SessionFacts> {
        Some(SessionFacts {
            state,
            started_at: t("2026-10-05T09:00:30+10:00"),
        })
    }

    #[test]
    fn decision_table() {
        use MarkerState::{Fresh, Nudged};
        use StopDecision::{AskForHandoff, Silent};
        let cases = [
            (
                "no marker",
                None,
                None,
                Some("2026-10-05T10:00:00+10:00"),
                Silent,
            ),
            (
                "already nudged",
                session(Nudged),
                None,
                Some("2026-10-05T10:00:00+10:00"),
                Silent,
            ),
            (
                "no session activity, or a marker without a baseline",
                session(Fresh),
                None,
                None,
                Silent,
            ),
            (
                "commit before the session",
                session(Fresh),
                None,
                Some("2026-10-05T08:00:00+10:00"),
                Silent,
            ),
            (
                "commit in the session's first minute",
                session(Fresh),
                None,
                Some("2026-10-05T09:00:59+10:00"),
                Silent,
            ),
            (
                "commit after start, no handoff",
                session(Fresh),
                None,
                Some("2026-10-05T09:01:00+10:00"),
                AskForHandoff,
            ),
            (
                "commit after start, older handoff",
                session(Fresh),
                Some(entry("2026-10-04T17:00:00+10:00")),
                Some("2026-10-05T09:30:00+10:00"),
                AskForHandoff,
            ),
            (
                "handoff after the last commit",
                session(Fresh),
                Some(entry("2026-10-05T10:05:00+10:00")),
                Some("2026-10-05T10:00:00+10:00"),
                Silent,
            ),
            (
                "commit in the handoff's minute",
                session(Fresh),
                Some(entry("2026-10-05T10:05:00+10:00")),
                Some("2026-10-05T10:05:45+10:00"),
                Silent,
            ),
            (
                "commit after the handoff",
                session(Fresh),
                Some(entry("2026-10-05T10:05:00+10:00")),
                Some("2026-10-05T10:06:00+10:00"),
                AskForHandoff,
            ),
            (
                "dirty tree edited after start",
                session(Fresh),
                None,
                Some("2026-10-05T09:45:12+10:00"),
                AskForHandoff,
            ),
            (
                "handoff in another offset",
                session(Fresh),
                Some(entry("2026-10-05T00:10:00+00:00")),
                Some("2026-10-05T10:09:00+10:00"),
                Silent,
            ),
            (
                "activity in another offset",
                session(Fresh),
                Some(entry("2026-10-05T10:05:00+10:00")),
                Some("2026-10-05T00:06:00+00:00"),
                AskForHandoff,
            ),
        ];
        for (name, session, last_handoff, activity, expected) in cases {
            let facts = StopFacts {
                session,
                last_handoff,
                activity: activity.map(t),
            };
            assert_eq!(decide(facts), expected, "{name}");
        }
    }
}
