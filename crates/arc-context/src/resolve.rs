use std::path::Path;

use arc_core::StoryId;
use arc_notes::{BranchName, NoteRelPath};

use crate::branch::story_id_in_branch;
use crate::git::GitHead;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Via {
    BranchId,
    LinkedNote(NoteRelPath),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedStory {
    pub id: StoryId,
    pub branch: BranchName,
    pub via: Via,
}

pub fn resolve(head: &GitHead, cache_dir: &Path) -> anyhow::Result<Option<ResolvedStory>> {
    let Some(branch) = head.branch() else {
        return Ok(None);
    };
    let found = match story_id_in_branch(branch) {
        Some(id) => Some((id, Via::BranchId)),
        None => {
            let conn = arc_index::open_for_reading(cache_dir)?;
            arc_index::story_for_branch(&conn, branch)?
                .map(|(id, note)| (id, Via::LinkedNote(note)))
        }
    };
    Ok(found.map(|(id, via)| ResolvedStory {
        id,
        branch: branch.clone(),
        via,
    }))
}
