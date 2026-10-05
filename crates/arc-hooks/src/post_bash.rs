use std::fmt;

use arc_context::git::{self, GitHead};
use arc_context::{LinkableBranch, ResolvedStory, StackLink, link_stacked_branch, resolve};
use arc_core::{StoryId, dbg_file};
use arc_notes::BranchName;
use arc_shortcut::Cache;

use crate::HookEnv;
use crate::command::{BranchCreation, ParentRef, branch_creations};
use crate::input::{PostToolUseInput, ToolCall};
use crate::output::ContextOutput;
use crate::repo::{self, Freshness};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Plan {
    Link {
        branch: LinkableBranch,
        parent: ResolvedStory,
    },
    Nudge(LinkableBranch),
    Nothing,
}

pub fn plan(
    new_story: Option<&ResolvedStory>,
    parent: Option<ResolvedStory>,
    branch: Option<LinkableBranch>,
) -> Plan {
    match (new_story, parent, branch) {
        (Some(_), _, _) | (None, _, None) => Plan::Nothing,
        (None, Some(parent), Some(branch)) => Plan::Link { branch, parent },
        (None, None, Some(branch)) => Plan::Nudge(branch),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Linked {
        branch: BranchName,
        story: StoryId,
        parent: BranchName,
    },
    Unlinked(BranchName),
    NoStoryNote {
        branch: BranchName,
        story: StoryId,
        parent: BranchName,
    },
    Silent,
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Outcome::Linked {
                branch,
                story,
                parent,
            } => write!(
                f,
                "arc: linked {branch} to sc-{story} (stacked on {parent})"
            ),
            Outcome::Unlinked(branch) => write!(
                f,
                "arc: new branch {branch} isn't linked to a Shortcut story. If this is story work, run `arc link <id>` with it checked out, or name it with `arc branch <id>`; otherwise ignore this."
            ),
            Outcome::NoStoryNote {
                branch,
                story,
                parent,
            } => write!(
                f,
                "arc: {branch} was created from {parent} (sc-{story}), but sc-{story} has no story note to link it in yet. If it is part of sc-{story}, run `arc link sc-{story}` with it checked out; otherwise ignore this."
            ),
            Outcome::Silent => Ok(()),
        }
    }
}

fn parent_branch(creation: &BranchCreation) -> anyhow::Result<Option<BranchName>> {
    match &creation.parent {
        ParentRef::StartPoint(start) => repo::branch_named_by(&creation.repo, start),
        ParentRef::PreviousCheckout => repo::branch_before_checkout(&creation.repo, &creation.new),
        ParentRef::CurrentHead => Ok(git::head(&creation.repo)?.branch().cloned()),
    }
}

fn story_of(branch: &BranchName, env: &HookEnv) -> anyhow::Result<Option<ResolvedStory>> {
    resolve(&GitHead::Branch(branch.clone()), &env.config.cache_dir)
}

fn handle(creation: &BranchCreation, env: &HookEnv, cache: &Cache) -> anyhow::Result<Outcome> {
    match repo::branch_freshness(&creation.repo, &creation.new, env.now)? {
        Freshness::Missing | Freshness::Stale => return Ok(Outcome::Silent),
        Freshness::Fresh | Freshness::Unlogged => {}
    }
    let new_story = story_of(&creation.new, env)?;
    let parent = match parent_branch(creation)? {
        Some(branch) => story_of(&branch, env)?,
        None => None,
    };
    let default_branch = git::default_branch(&creation.repo)?;
    let linkable = LinkableBranch::parse(creation.new.clone(), default_branch.as_ref()).ok();
    Ok(match plan(new_story.as_ref(), parent, linkable) {
        Plan::Nothing => Outcome::Silent,
        Plan::Nudge(branch) => Outcome::Unlinked(branch.into_branch()),
        Plan::Link { branch, parent } => {
            match link_stacked_branch(&env.config, cache, &parent, &branch)? {
                StackLink::Linked(..) => Outcome::Linked {
                    branch: branch.into_branch(),
                    story: parent.id,
                    parent: parent.branch,
                },
                StackLink::NoNote => Outcome::NoStoryNote {
                    branch: branch.into_branch(),
                    story: parent.id,
                    parent: parent.branch,
                },
            }
        }
    })
}

pub fn run(
    input: PostToolUseInput,
    env: &HookEnv,
    cache: &Cache,
) -> anyhow::Result<Option<String>> {
    let ToolCall::Bash { tool_input } = input.tool else {
        return Ok(None);
    };
    let messages: Vec<String> =
        branch_creations(&tool_input.command, input.cwd.path(), env.home.as_deref())
            .iter()
            .filter_map(|creation| match handle(creation, env, cache) {
                Ok(Outcome::Silent) => None,
                Ok(outcome) => Some(outcome.to_string()),
                Err(e) => {
                    dbg_file!("arc hook post-bash: {}: {e:#}", creation.new);
                    None
                }
            })
            .collect();
    if messages.is_empty() {
        return Ok(None);
    }
    let output = ContextOutput::post_tool_use(messages.join("\n"));
    Ok(Some(serde_json::to_string(&output)?))
}
