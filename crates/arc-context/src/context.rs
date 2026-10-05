use std::fmt;
use std::path::Path;

use arc_core::StoryId;
use arc_index::Connection;
use arc_notes::{BranchName, NoteRelPath, Todo, TodoSource};
use arc_shortcut::Cache;
use arc_shortcut::api::story::Story;
use chrono::NaiveDate;
use serde::Serialize;

use crate::branch::canonical_branch;
use crate::resolve::{ResolvedStory, Via};
use crate::sessions::latest_sessions;

pub const SESSION_COUNT: usize = 3;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StoryContext {
    pub story: ContextStory,
    pub branch: BranchName,
    pub resolved_by: ResolvedBy,
    pub canonical_branch: Option<BranchName>,
    pub note: Option<NoteRelPath>,
    pub sessions: Vec<String>,
    pub open_todos: Vec<OpenTodo>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ContextStory {
    pub id: StoryId,
    #[serde(flatten)]
    pub details: Option<StoryDetails>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StoryDetails {
    pub name: String,
    pub state: Option<String>,
    #[serde(rename = "type")]
    pub story_type: String,
    pub url: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolvedBy {
    BranchId,
    LinkedNote,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct OpenTodo {
    pub text: String,
    pub date: Option<NaiveDate>,
    pub line: u32,
}

impl From<&Via> for ResolvedBy {
    fn from(via: &Via) -> Self {
        match via {
            Via::BranchId => ResolvedBy::BranchId,
            Via::LinkedNote(_) => ResolvedBy::LinkedNote,
        }
    }
}

fn find_note(
    resolved: &ResolvedStory,
    conn: &Connection,
    notes_dir: &Path,
) -> anyhow::Result<Option<(NoteRelPath, String)>> {
    if let Via::LinkedNote(rel) = &resolved.via
        && let Some(body) = arc_index::read_note(notes_dir, rel)?
    {
        return Ok(Some((rel.clone(), body)));
    }
    arc_index::story_note(conn, notes_dir, resolved.id)
}

fn open_todos_in(note: &NoteRelPath, todos: &[Todo]) -> Vec<OpenTodo> {
    todos
        .iter()
        .filter(|t| !t.completed)
        .filter_map(|t| match &t.source {
            TodoSource::NoteParsed { file, line, .. } if file == note => Some(OpenTodo {
                text: t.text.clone(),
                date: t.date,
                line: *line,
            }),
            TodoSource::NoteParsed { .. } | TodoSource::Manual => None,
        })
        .collect()
}

pub fn build_context(
    resolved: ResolvedStory,
    story: Option<&Story>,
    cache: &Cache,
    conn: &Connection,
    notes_dir: &Path,
    todos: &[Todo],
) -> anyhow::Result<StoryContext> {
    let note = find_note(&resolved, conn, notes_dir)?;
    let canonical = match (cache.user(), story) {
        (Some(user), Some(story)) => Some(canonical_branch(
            &user.mention_name,
            resolved.id,
            &story.name,
            None,
        )?),
        _ => None,
    };
    let details = story.map(|s| StoryDetails {
        name: s.name.clone(),
        state: cache.state_name(s.workflow_state_id).map(str::to_owned),
        story_type: s.story_type.clone(),
        url: s.app_url.clone(),
    });
    Ok(StoryContext {
        story: ContextStory {
            id: resolved.id,
            details,
        },
        resolved_by: ResolvedBy::from(&resolved.via),
        branch: resolved.branch,
        canonical_branch: canonical,
        sessions: note
            .as_ref()
            .map(|(_, body)| latest_sessions(body, SESSION_COUNT))
            .unwrap_or_default(),
        open_todos: note
            .as_ref()
            .map(|(rel, _)| open_todos_in(rel, todos))
            .unwrap_or_default(),
        note: note.map(|(rel, _)| rel),
    })
}

impl fmt::Display for ResolvedBy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ResolvedBy::BranchId => "story id in branch name",
            ResolvedBy::LinkedNote => "linked in the story note's branches",
        })
    }
}

impl fmt::Display for StoryContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let id = self.story.id;
        match &self.story.details {
            Some(d) => {
                writeln!(f, "Shortcut story sc-{id}: {}", d.name)?;
                let facts: Vec<String> = [
                    d.state.as_ref().map(|s| format!("State: {s}")),
                    (!d.story_type.is_empty()).then(|| format!("Type: {}", d.story_type)),
                ]
                .into_iter()
                .flatten()
                .collect();
                if !facts.is_empty() {
                    writeln!(f, "{}", facts.join(" · "))?;
                }
                writeln!(f, "URL: {}", d.url)?;
            }
            None => writeln!(f, "Shortcut story sc-{id} (details unavailable offline)")?,
        }
        writeln!(f, "Branch: {} ({})", self.branch, self.resolved_by)?;
        if let Some(canonical) = &self.canonical_branch {
            writeln!(f, "Canonical branch: {canonical}")?;
        }
        match &self.note {
            Some(note) => writeln!(f, "Note: {note}")?,
            None => writeln!(f, "Note: none")?,
        }
        if !self.open_todos.is_empty() {
            writeln!(f, "\nOpen todos:")?;
            for todo in &self.open_todos {
                match todo.date {
                    Some(date) => writeln!(f, "- [ ] {} (due {date})", todo.text)?,
                    None => writeln!(f, "- [ ] {}", todo.text)?,
                }
            }
        }
        if !self.sessions.is_empty() {
            writeln!(f, "\nRecent sessions:")?;
            for session in &self.sessions {
                writeln!(f, "\n{session}")?;
            }
        }
        Ok(())
    }
}
