use std::path::Path;

use anyhow::{Context, bail};
use arc_core::{Config, StoryId};
use arc_index::Connection;
use arc_notes::{BranchName, NoteRelPath};
use arc_shortcut::Cache;
use arc_shortcut::api::story::Story;

pub mod branch;
pub mod context;
pub mod git;
pub mod note;
pub mod resolve;
pub mod section;
pub mod sessions;
pub mod story;
pub mod todos;

pub use branch::{LinkableBranch, StackableBranch, canonical_branch, story_id_in_branch};
pub use context::{ContextStory, OpenTodo, ResolvedBy, StoryContext, StoryDetails, build_context};
pub use git::GitHead;
pub use note::{
    LinkOutcome, NoteLinks, NoteRename, StoryNote, Vault, create_story_note, link_branch,
    rename_linked_branch, write_breadcrumb, write_session,
};
pub use resolve::{ResolvedStory, Via, resolve};
pub use section::{Newline, SectionHeading, append_to_end, append_to_section};
pub use sessions::{
    Breadcrumb, EntryHeader, EntryKind, EntryText, Handoff, OneLine, SessionEntry, SessionTime,
    activity_since, append_breadcrumb, append_session, context_sessions, latest_handoff_at,
    latest_session_at, latest_sessions,
};
pub use story::{lookup_story, lookup_story_backing_off, lookup_user};
pub use todos::{ListedTodo, TodoListing, add_todo, complete_todo, open_todos, todo_listing};

pub fn resolve_dir(dir: &Path, config: &Config) -> anyhow::Result<Option<ResolvedStory>> {
    resolve(&git::head(dir)?, &config.cache_dir)
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
) -> anyhow::Result<T> {
    tokio::task::spawn_blocking(work).await?
}

pub async fn current_context(
    dir: &Path,
    config: &Config,
    cache: &mut Cache,
) -> anyhow::Result<Option<StoryContext>> {
    let (owned_dir, owned_config) = (dir.to_path_buf(), config.clone());
    let Some(resolved) = blocking(move || resolve_dir(&owned_dir, &owned_config)).await? else {
        return Ok(None);
    };
    let story = lookup_story_backing_off(cache, &config.api_token, &config.cache_dir, resolved.id)
        .await
        .ok();
    let todos = arc_notes::todos::load_todos(&config.cache_dir).await;
    let (cache_dir, notes_dir, lookup) = (
        config.cache_dir.clone(),
        config.notes_dir.clone(),
        resolved.clone(),
    );
    let note = blocking(move || {
        let conn = arc_index::open_for_reading(&cache_dir)?;
        context::find_note(&lookup, &conn, &notes_dir)
    })
    .await?;
    Ok(Some(context::assemble(
        resolved,
        story.as_ref(),
        cache,
        note,
        &todos,
    )))
}

pub async fn branch_for_story(
    config: &Config,
    cache: &mut Cache,
    id: StoryId,
    suffix: Option<&str>,
) -> anyhow::Result<BranchName> {
    let user = lookup_user(cache, &config.api_token)
        .await
        .context("couldn't load your Shortcut mention name")?;
    let story = lookup_story(cache, &config.api_token, id)
        .await
        .with_context(|| format!("couldn't load story {id}"))?;
    canonical_branch(&user.mention_name, id, &story.name, suffix)
}

fn linked_note(
    conn: &Connection,
    branch: &BranchName,
    id: StoryId,
) -> anyhow::Result<Option<NoteRelPath>> {
    if let Some(named) = story_id_in_branch(branch).filter(|&named| named != id) {
        bail!("branch '{branch}' already names story {named}");
    }
    match arc_index::story_for_branch(conn, branch)? {
        Some((other, note)) if other != id => {
            bail!("branch '{branch}' is already linked to story {other} in '{note}'")
        }
        Some((_, note)) => Ok(Some(note)),
        None => Ok(None),
    }
}

pub async fn link_current_branch(
    dir: &Path,
    config: &Config,
    cache: &mut Cache,
    id: StoryId,
) -> anyhow::Result<(BranchName, NoteRelPath, LinkOutcome)> {
    let branch = git::head(dir)?.require_branch()?.clone();
    let branch = LinkableBranch::parse(branch, git::default_branch(dir)?.as_ref())?;
    let conn = arc_index::open(&config.cache_dir)?;
    let vault = Vault::new(&conn, &config.notes_dir, &config.cache_dir);
    linked_note(&conn, branch.as_branch(), id)?;
    let note = match arc_index::story_note(&conn, &config.notes_dir, id)? {
        Some((rel, _)) => rel,
        None => {
            let story = lookup_story(cache, &config.api_token, id)
                .await
                .with_context(|| format!("couldn't load story {id} to create its note"))?;
            let links = NoteLinks::for_story(cache, &story);
            match create_story_note(&vault, &story, links, "")? {
                StoryNote::Created { rel, .. } | StoryNote::Existing(rel) => rel,
            }
        }
    };
    let outcome = link_branch(&vault, &note, &branch)?;
    Ok((branch.into_branch(), note, outcome))
}

pub fn story_note_for(
    resolved: &ResolvedStory,
    config: &Config,
) -> anyhow::Result<Option<(NoteRelPath, String)>> {
    let conn = arc_index::open_for_reading(&config.cache_dir)?;
    context::find_note(resolved, &conn, &config.notes_dir)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StackLink {
    Linked(NoteRelPath, LinkOutcome),
    NoNote,
}

pub fn link_stacked_branch(
    config: &Config,
    cache: &Cache,
    parent: &ResolvedStory,
    branch: &StackableBranch,
) -> anyhow::Result<StackLink> {
    let branch = branch.as_linkable();
    let conn = arc_index::open(&config.cache_dir)?;
    let vault = Vault::new(&conn, &config.notes_dir, &config.cache_dir);
    linked_note(&conn, branch.as_branch(), parent.id)?;
    let note = match context::find_note(parent, &conn, &config.notes_dir)? {
        Some((rel, _)) => rel,
        None => match cache.find_story(parent.id) {
            Some(story) => {
                let links = NoteLinks::for_story(cache, story);
                create_story_note(&vault, story, links, "")?.rel().clone()
            }
            None => return Ok(StackLink::NoNote),
        },
    };
    let outcome = link_branch(&vault, &note, branch)?;
    Ok(StackLink::Linked(note, outcome))
}

#[derive(Debug)]
pub struct LoggedSession {
    pub story: StoryId,
    pub head: GitHead,
    pub note: StoryNote,
    pub entry: SessionEntry,
}

struct SessionTarget {
    story: StoryId,
    head: GitHead,
    branch: Option<BranchName>,
}

enum Located {
    Noted(SessionTarget, NoteRelPath),
    Unnoted(SessionTarget),
}

fn locate_session(dir: &Path, config: &Config, story: Option<StoryId>) -> anyhow::Result<Located> {
    let head = git::head(dir)?;
    let (id, resolved) = match story {
        Some(id) => (
            id,
            resolve(&head, &config.cache_dir)
                .ok()
                .flatten()
                .filter(|r| r.id == id),
        ),
        None => {
            let resolved = resolve(&head, &config.cache_dir)?
                .context("the current branch doesn't resolve to a story; pass a story id")?;
            (resolved.id, Some(resolved))
        }
    };
    let conn = arc_index::open(&config.cache_dir)?;
    let existing = match &resolved {
        Some(resolved) => context::find_note(resolved, &conn, &config.notes_dir)?,
        None => arc_index::story_note(&conn, &config.notes_dir, id)?,
    };
    let target = SessionTarget {
        story: id,
        head,
        branch: resolved.map(|r| r.branch),
    };
    Ok(match existing {
        Some((rel, _)) => Located::Noted(target, rel),
        None => Located::Unnoted(target),
    })
}

fn create_note_for(config: &Config, story: &Story, links: NoteLinks) -> anyhow::Result<StoryNote> {
    let conn = arc_index::open(&config.cache_dir)?;
    create_story_note(
        &Vault::new(&conn, &config.notes_dir, &config.cache_dir),
        story,
        links,
        "",
    )
}

fn write_entry(config: &Config, rel: &NoteRelPath, entry: &SessionEntry) -> anyhow::Result<()> {
    let conn = arc_index::open(&config.cache_dir)?;
    write_session(
        &Vault::new(&conn, &config.notes_dir, &config.cache_dir),
        rel,
        entry,
    )
}

pub async fn log_session(
    dir: &Path,
    config: &Config,
    cache: &mut Cache,
    story: Option<StoryId>,
    handoff: Handoff,
) -> anyhow::Result<LoggedSession> {
    let (owned_dir, owned_config) = (dir.to_path_buf(), config.clone());
    let located = blocking(move || locate_session(&owned_dir, &owned_config, story)).await?;
    let (target, note) = match located {
        Located::Noted(target, rel) => (target, StoryNote::Existing(rel)),
        Located::Unnoted(target) => {
            let id = target.story;
            let story = lookup_story(cache, &config.api_token, id)
                .await
                .with_context(|| format!("couldn't load story {id} to create its note"))?;
            let links = NoteLinks::for_story(cache, &story);
            let owned_config = config.clone();
            let note = blocking(move || create_note_for(&owned_config, &story, links)).await?;
            (target, note)
        }
    };
    let entry = SessionEntry {
        at: SessionTime::now(),
        branch: target.branch,
        handoff,
    };
    let (owned_config, rel) = (config.clone(), note.rel().clone());
    let entry = blocking(move || write_entry(&owned_config, &rel, &entry).map(|()| entry)).await?;
    Ok(LoggedSession {
        story: target.story,
        head: target.head,
        note,
        entry,
    })
}

#[derive(Debug)]
pub struct Renamed {
    pub from: BranchName,
    pub to: BranchName,
    pub note: NoteRename,
}

pub async fn rename_to_canonical(
    dir: &Path,
    config: &Config,
    cache: &mut Cache,
    id: StoryId,
) -> anyhow::Result<Renamed> {
    let current = git::head(dir)?.require_branch()?.clone();
    let canonical = branch_for_story(config, cache, id, None).await?;
    if canonical == current {
        bail!("already on the canonical branch '{canonical}'");
    }
    let conn = arc_index::open(&config.cache_dir)?;
    let vault = Vault::new(&conn, &config.notes_dir, &config.cache_dir);
    let linked = linked_note(&conn, &current, id)?;
    git::rename_current_branch(dir, &canonical)?;
    let note = match linked {
        Some(rel) => rename_linked_branch(&vault, &rel, &current, &canonical),
        None => NoteRename::NotListed,
    };
    Ok(Renamed {
        from: current,
        to: canonical,
        note,
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::process::Command;

    use arc_notes::note::frontmatter::Frontmatter;
    use arc_notes::todos::Todo;
    use arc_shortcut::api::story::Story;
    use arc_shortcut::api::user::{CurrentUser, MemberId, MentionName};
    use serde_json::json;
    use uuid::Uuid;

    use super::*;

    struct Env {
        _tmp: tempfile::TempDir,
        repo: PathBuf,
        config: Config,
    }

    fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    }

    fn run_git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn env_on(branch: &str) -> Option<Env> {
        if !git_available() {
            eprintln!("git not found; skipping");
            return None;
        }
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        run_git(&repo, &["init", "-q"]);
        run_git(
            &repo,
            &["symbolic-ref", "HEAD", &format!("refs/heads/{branch}")],
        );
        let config = Config {
            notes_dir: tmp.path().join("notes"),
            cache_dir: tmp.path().join("cache"),
            api_token: String::new(),
            editor: String::new(),
            repositories_directory: tmp.path().to_path_buf(),
            mux: Default::default(),
        };
        std::fs::create_dir_all(config.notes_dir.join("stories")).unwrap();
        Some(Env {
            _tmp: tmp,
            repo,
            config,
        })
    }

    fn rel(s: &str) -> NoteRelPath {
        NoteRelPath::parse(s).unwrap()
    }

    fn br(s: &str) -> BranchName {
        BranchName::parse(s).unwrap()
    }

    fn write_note(env: &Env, path: &str, content: &str) {
        std::fs::write(env.config.notes_dir.join(path), content).unwrap();
        let conn = arc_index::open(&env.config.cache_dir).unwrap();
        arc_index::reindex_note(&conn, &rel(path), content).unwrap();
    }

    fn cache_for(env: &Env) -> Cache {
        let mut cache = Cache::default().with_cache_dir(env.config.cache_dir.clone());
        cache.set_user(CurrentUser {
            id: MemberId::from(Uuid::nil()),
            mention_name: MentionName::parse("tenzin").unwrap(),
        });
        cache
    }

    #[test]
    fn head_reports_branch_detached_and_no_repo() {
        let Some(env) = env_on("me/sc-12/x") else {
            return;
        };
        assert_eq!(
            git::head(&env.repo).unwrap(),
            GitHead::Branch(br("me/sc-12/x"))
        );
        run_git(&env.repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
        run_git(&env.repo, &["checkout", "-q", "--detach"]);
        assert_eq!(git::head(&env.repo).unwrap(), GitHead::Detached);
        let outside = tempfile::tempdir().unwrap();
        assert_eq!(git::head(outside.path()).unwrap(), GitHead::NotARepo);
    }

    #[test]
    fn resolution_prefers_branch_id_then_link_then_nothing() {
        let Some(env) = env_on("me/sc-12/x") else {
            return;
        };
        write_note(
            &env,
            "stories/other.md",
            "---\nstory_id: sc-99\nbranches: [me/sc-12/x, linked, main]\n---\n",
        );

        assert_eq!(
            resolve_dir(&env.repo, &env.config).unwrap(),
            Some(ResolvedStory {
                id: 12.into(),
                branch: br("me/sc-12/x"),
                via: Via::BranchId
            })
        );

        run_git(&env.repo, &["symbolic-ref", "HEAD", "refs/heads/linked"]);
        assert_eq!(
            resolve_dir(&env.repo, &env.config).unwrap(),
            Some(ResolvedStory {
                id: 99.into(),
                branch: br("linked"),
                via: Via::LinkedNote(rel("stories/other.md")),
            })
        );

        run_git(&env.repo, &["symbolic-ref", "HEAD", "refs/heads/unlinked"]);
        assert_eq!(resolve_dir(&env.repo, &env.config).unwrap(), None);

        let outside = tempfile::tempdir().unwrap();
        assert_eq!(resolve_dir(outside.path(), &env.config).unwrap(), None);
    }

    #[test]
    fn detached_head_resolves_nothing() {
        let Some(env) = env_on("me/sc-12/x") else {
            return;
        };
        run_git(&env.repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
        run_git(&env.repo, &["checkout", "-q", "--detach"]);
        assert_eq!(resolve_dir(&env.repo, &env.config).unwrap(), None);
    }

    fn story_seven() -> Story {
        Story {
            id: 7.into(),
            name: "Story seven".into(),
            app_url: "https://app.shortcut.com/x/story/7".into(),
            workflow_state_id: Some(500.into()),
            story_type: "feature".into(),
            ..Default::default()
        }
    }

    const SEVEN_NOTE: &str = "---\nid: story-seven\nstory_id: sc-7\n---\n\n# Story seven\n\n- [ ] ship it @07-10-2026\n- [x] done already\n- [ ] write tests\n\n## Sessions\n\n### 2026-10-01 · one\n**Done:** a\n\n### 2026-10-02 · two\n**Done:** b\n\n### 2026-10-03 · three\n**Done:** c\n\n### 2026-10-04 · four\n**Done:** d\n**Next:** e\n";

    fn note_todos(path: &str, content: &str) -> Vec<Todo> {
        arc_notes::scanner::parse_note(content, &rel(path))
            .into_iter()
            .chain(arc_notes::scanner::parse_note("- [ ] added by hand\n", &NoteRelPath::todos()))
            .collect()
    }

    fn golden_context() -> StoryContext {
        let tmp = tempfile::tempdir().unwrap();
        let notes = tmp.path().join("notes");
        std::fs::create_dir_all(notes.join("stories")).unwrap();
        std::fs::write(notes.join("stories/story-seven.md"), SEVEN_NOTE).unwrap();
        let conn = arc_index::open(&tmp.path().join("cache")).unwrap();
        let mut cache = Cache::default();
        cache
            .workflow_states
            .insert(500.into(), "In Progress".into());
        cache.set_user(CurrentUser {
            id: MemberId::from(Uuid::nil()),
            mention_name: MentionName::parse("tenzin").unwrap(),
        });
        let mut todos = note_todos("stories/story-seven.md", SEVEN_NOTE);
        todos.extend(note_todos("stories/elsewhere.md", "- [ ] not this note\n"));
        let resolved = ResolvedStory {
            id: 7.into(),
            branch: br("feature-x"),
            via: Via::BranchId,
        };
        build_context(
            resolved,
            Some(&story_seven()),
            &cache,
            &conn,
            &notes,
            &todos,
        )
        .unwrap()
    }

    #[test]
    fn golden_context_json() {
        assert_eq!(
            serde_json::to_value(golden_context()).unwrap(),
            json!({
                "story": {
                    "id": 7,
                    "name": "Story seven",
                    "state": "In Progress",
                    "type": "feature",
                    "url": "https://app.shortcut.com/x/story/7"
                },
                "branch": "feature-x",
                "resolved_by": "branch_id",
                "canonical_branch": "tenzin/sc-7/story-seven",
                "note": "stories/story-seven.md",
                "sessions": [
                    "### 2026-10-02 · two\n**Done:** b",
                    "### 2026-10-03 · three\n**Done:** c",
                    "### 2026-10-04 · four\n**Done:** d\n**Next:** e"
                ],
                "open_todos": [
                    {"text": "ship it", "date": "2026-10-07", "line": 8},
                    {"text": "write tests", "date": null, "line": 10}
                ]
            })
        );
    }

    #[test]
    fn golden_context_text() {
        assert_eq!(
            golden_context().to_string(),
            "Shortcut story sc-7: Story seven\n\
             State: In Progress · Type: feature\n\
             URL: https://app.shortcut.com/x/story/7\n\
             Branch: feature-x (story id in branch name)\n\
             Canonical branch: tenzin/sc-7/story-seven\n\
             Note: stories/story-seven.md\n\
             \n\
             Open todos:\n\
             - [ ] ship it (due 2026-10-07)\n\
             - [ ] write tests\n\
             \n\
             Recent sessions:\n\
             \n\
             ### 2026-10-02 · two\n\
             **Done:** b\n\
             \n\
             ### 2026-10-03 · three\n\
             **Done:** c\n\
             \n\
             ### 2026-10-04 · four\n\
             **Done:** d\n\
             **Next:** e\n"
        );
    }

    #[test]
    fn golden_context_without_story_details_or_note() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = arc_index::open(tmp.path()).unwrap();
        let resolved = ResolvedStory {
            id: 8.into(),
            branch: br("me/sc-8"),
            via: Via::BranchId,
        };
        let ctx = build_context(resolved, None, &Cache::default(), &conn, tmp.path(), &[]).unwrap();
        assert_eq!(
            serde_json::to_value(&ctx).unwrap(),
            json!({
                "story": {"id": 8},
                "branch": "me/sc-8",
                "resolved_by": "branch_id",
                "canonical_branch": null,
                "note": null,
                "sessions": [],
                "open_todos": []
            })
        );
        assert_eq!(
            ctx.to_string(),
            "Shortcut story sc-8 (details unavailable offline)\nBranch: me/sc-8 (story id in branch name)\nNote: none\n"
        );
    }

    #[test]
    fn linked_note_context_reads_the_linking_note() {
        let tmp = tempfile::tempdir().unwrap();
        let notes = tmp.path().join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        std::fs::write(
            notes.join("work.md"),
            "---\nstory_id: sc-7\n---\n## Sessions\n### s\n",
        )
        .unwrap();
        let conn = arc_index::open(&tmp.path().join("cache")).unwrap();
        let resolved = ResolvedStory {
            id: 7.into(),
            branch: br("linked"),
            via: Via::LinkedNote(rel("work.md")),
        };
        let ctx = build_context(resolved, None, &Cache::default(), &conn, &notes, &[]).unwrap();
        assert_eq!(ctx.resolved_by, ResolvedBy::LinkedNote);
        assert_eq!(ctx.note, Some(rel("work.md")));
        assert_eq!(ctx.sessions, vec!["### s"]);
    }

    #[tokio::test]
    async fn current_context_is_none_off_story_branches() {
        let Some(env) = env_on("main") else { return };
        let mut cache = cache_for(&env);
        assert_eq!(
            current_context(&env.repo, &env.config, &mut cache)
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn current_context_uses_cached_story() {
        let Some(env) = env_on("tenzin/sc-7/story-seven") else {
            return;
        };
        write_note(&env, "stories/story-seven.md", SEVEN_NOTE);
        let mut cache = cache_for(&env);
        cache.upsert_story(story_seven());
        let ctx = current_context(&env.repo, &env.config, &mut cache)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ctx.story.details.unwrap().name, "Story seven");
        assert_eq!(ctx.note, Some(rel("stories/story-seven.md")));
        assert_eq!(ctx.canonical_branch, Some(br("tenzin/sc-7/story-seven")));
        assert_eq!(ctx.sessions.len(), 3);
    }

    #[tokio::test]
    async fn link_creates_missing_note_then_updates_it() {
        let Some(env) = env_on("feature-x") else {
            return;
        };
        let mut cache = cache_for(&env);
        cache.upsert_story(story_seven());

        let (branch, note, outcome) =
            link_current_branch(&env.repo, &env.config, &mut cache, 7.into())
                .await
                .unwrap();
        assert_eq!(
            (branch, &note, outcome),
            (
                br("feature-x"),
                &rel("stories/story-seven.md"),
                LinkOutcome::Added
            )
        );
        let body = std::fs::read_to_string(note.abs(&env.config.notes_dir)).unwrap();
        assert_eq!(
            Frontmatter::from_note(&body).unwrap().branches,
            vec!["feature-x"]
        );
        assert!(body.ends_with("---\n\n# Story seven\n"));

        let resolved = resolve_dir(&env.repo, &env.config).unwrap().unwrap();
        assert_eq!(resolved.via, Via::LinkedNote(note.clone()));

        run_git(&env.repo, &["symbolic-ref", "HEAD", "refs/heads/stacked"]);
        let (_, again, outcome) = link_current_branch(&env.repo, &env.config, &mut cache, 7.into())
            .await
            .unwrap();
        assert_eq!((again, outcome), (note.clone(), LinkOutcome::Added));
        let body = std::fs::read_to_string(note.abs(&env.config.notes_dir)).unwrap();
        assert_eq!(
            Frontmatter::from_note(&body).unwrap().branches,
            vec!["feature-x", "stacked"]
        );

        let (_, _, outcome) = link_current_branch(&env.repo, &env.config, &mut cache, 7.into())
            .await
            .unwrap();
        assert_eq!(outcome, LinkOutcome::AlreadyLinked);
    }

    #[tokio::test]
    async fn link_updates_existing_unindexed_note() {
        let Some(env) = env_on("feature-y") else {
            return;
        };
        let content = "---\nid: renamed\nstory_id: sc-7\nbranches:\n  - old\n---\nbody\n";
        std::fs::write(env.config.notes_dir.join("stories/renamed.md"), content).unwrap();
        let mut cache = cache_for(&env);
        let (_, note, outcome) = link_current_branch(&env.repo, &env.config, &mut cache, 7.into())
            .await
            .unwrap();
        assert_eq!(
            (note.clone(), outcome),
            (rel("stories/renamed.md"), LinkOutcome::Added)
        );
        assert_eq!(
            std::fs::read_to_string(note.abs(&env.config.notes_dir)).unwrap(),
            "---\nid: renamed\nstory_id: sc-7\nbranches:\n  - old\n  - feature-y\n---\nbody\n"
        );
    }

    #[tokio::test]
    async fn link_refuses_conflicting_branches_and_bad_heads() {
        let Some(env) = env_on("me/sc-12/x") else {
            return;
        };
        let mut cache = cache_for(&env);
        let err = link_current_branch(&env.repo, &env.config, &mut cache, 7.into())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("already names story 12"), "{err}");

        write_note(
            &env,
            "stories/other.md",
            "---\nstory_id: sc-99\nbranches: [taken]\n---\n",
        );
        run_git(&env.repo, &["symbolic-ref", "HEAD", "refs/heads/taken"]);
        let err = link_current_branch(&env.repo, &env.config, &mut cache, 7.into())
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("already linked to story 99"),
            "{err}"
        );

        let outside = tempfile::tempdir().unwrap();
        let err = link_current_branch(outside.path(), &env.config, &mut cache, 7.into())
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("not inside a git repository"),
            "{err}"
        );

        run_git(&env.repo, &["symbolic-ref", "HEAD", "refs/heads/main"]);
        run_git(&env.repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
        run_git(&env.repo, &["checkout", "-q", "--detach"]);
        let err = link_current_branch(&env.repo, &env.config, &mut cache, 7.into())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("detached"), "{err}");
    }

    #[tokio::test]
    async fn rename_moves_to_canonical_once() {
        let Some(env) = env_on("wip") else { return };
        run_git(&env.repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
        let mut cache = cache_for(&env);
        cache.upsert_story(story_seven());
        let renamed = rename_to_canonical(&env.repo, &env.config, &mut cache, 7.into())
            .await
            .unwrap();
        assert_eq!(
            (renamed.from, renamed.to.clone()),
            (br("wip"), br("tenzin/sc-7/story-seven"))
        );
        assert!(
            matches!(renamed.note, NoteRename::NotListed),
            "{:?}",
            renamed.note
        );
        assert_eq!(git::head(&env.repo).unwrap(), GitHead::Branch(renamed.to));
        let err = rename_to_canonical(&env.repo, &env.config, &mut cache, 7.into())
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("already on the canonical branch"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn branch_for_story_uses_cached_user_and_story() {
        let Some(env) = env_on("main") else { return };
        let mut cache = cache_for(&env);
        cache.upsert_story(story_seven());
        assert_eq!(
            branch_for_story(&env.config, &mut cache, 7.into(), Some("v2"))
                .await
                .unwrap(),
            br("tenzin/sc-7/story-seven-v2")
        );
    }

    fn handoff(done: &str) -> Handoff {
        Handoff {
            done: EntryText::parse(done).unwrap(),
            state: EntryText::parse("s").unwrap(),
            next: EntryText::parse("n").unwrap(),
            questions: None,
        }
    }

    #[tokio::test]
    async fn log_session_creates_the_note_for_the_branch_story() {
        let Some(env) = env_on("tenzin/sc-7/story-seven") else {
            return;
        };
        let mut cache = cache_for(&env);
        cache.upsert_story(story_seven());
        let logged = log_session(&env.repo, &env.config, &mut cache, None, handoff("one"))
            .await
            .unwrap();
        assert_eq!(logged.story, 7.into());
        assert!(
            matches!(logged.note, StoryNote::Created { ref rel, .. } if *rel == "stories/story-seven.md"),
            "{:?}",
            logged.note
        );
        assert_eq!(logged.entry.branch, Some(br("tenzin/sc-7/story-seven")));

        let logged = log_session(&env.repo, &env.config, &mut cache, None, handoff("two"))
            .await
            .unwrap();
        assert_eq!(
            logged.note,
            StoryNote::Existing(rel("stories/story-seven.md"))
        );
        let body =
            std::fs::read_to_string(env.config.notes_dir.join("stories/story-seven.md")).unwrap();
        let sessions = latest_sessions(&body, 5);
        assert_eq!(sessions.len(), 2);
        assert!(sessions[0].contains("**Done:** one"), "{body}");
        assert_eq!(sessions[1], logged.entry.to_string());
        let conn = arc_index::open(&env.config.cache_dir).unwrap();
        assert_eq!(
            arc_index::note_for_story(&conn, 7.into()).unwrap(),
            Some(rel("stories/story-seven.md"))
        );
    }

    #[tokio::test]
    async fn log_session_uses_explicit_ids_and_linking_notes() {
        let Some(env) = env_on("linked") else { return };
        let mut cache = cache_for(&env);
        let err = log_session(&env.repo, &env.config, &mut cache, None, handoff("x"))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("pass a story id"), "{err}");

        write_note(
            &env,
            "stories/seven.md",
            "---\nstory_id: sc-7\n---\n# Seven\n",
        );
        let logged = log_session(
            &env.repo,
            &env.config,
            &mut cache,
            Some(7.into()),
            handoff("x"),
        )
        .await
        .unwrap();
        assert_eq!(logged.note, StoryNote::Existing(rel("stories/seven.md")));
        assert_eq!(logged.entry.branch, None);
        assert_eq!(logged.head, GitHead::Branch(br("linked")));

        write_note(
            &env,
            "work.md",
            "---\nstory_id: sc-7\nbranches: [linked]\n---\n# Work\n",
        );
        let logged = log_session(&env.repo, &env.config, &mut cache, None, handoff("y"))
            .await
            .unwrap();
        assert_eq!(logged.note, StoryNote::Existing(rel("work.md")));
        let body = std::fs::read_to_string(env.config.notes_dir.join("work.md")).unwrap();
        assert_eq!(
            body,
            format!(
                "---\nstory_id: sc-7\nbranches: [linked]\n---\n# Work\n\n## Sessions\n\n{}\n",
                logged.entry
            )
        );

        let logged = log_session(
            &env.repo,
            &env.config,
            &mut cache,
            Some(7.into()),
            handoff("linked"),
        )
        .await
        .unwrap();
        assert_eq!(logged.note, StoryNote::Existing(rel("work.md")));
        assert_eq!(logged.entry.branch, Some(br("linked")));

        run_git(
            &env.repo,
            &["symbolic-ref", "HEAD", "refs/heads/me/sc-12/x"],
        );
        let logged = log_session(
            &env.repo,
            &env.config,
            &mut cache,
            Some(7.into()),
            handoff("elsewhere"),
        )
        .await
        .unwrap();
        assert_eq!(logged.entry.branch, None);
        assert_eq!(logged.head, GitHead::Branch(br("me/sc-12/x")));
        assert_eq!(logged.note, StoryNote::Existing(rel("stories/seven.md")));

        let outside = tempfile::tempdir().unwrap();
        let logged = log_session(
            outside.path(),
            &env.config,
            &mut cache,
            Some(7.into()),
            handoff("z"),
        )
        .await
        .unwrap();
        assert_eq!(logged.entry.branch, None);
        assert_eq!(logged.head, GitHead::NotARepo);
        assert_eq!(logged.note, StoryNote::Existing(rel("stories/seven.md")));
    }

    #[test]
    fn head_ignores_same_named_tags() {
        let Some(env) = env_on("x") else { return };
        run_git(&env.repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
        run_git(&env.repo, &["tag", "x"]);
        assert_eq!(git::head(&env.repo).unwrap(), GitHead::Branch(br("x")));
    }

    #[test]
    fn branch_id_resolution_never_touches_the_index() {
        let Some(env) = env_on("me/sc-12/x") else {
            return;
        };
        assert_eq!(
            resolve_dir(&env.repo, &env.config).unwrap().unwrap().id,
            12.into()
        );
        run_git(&env.repo, &["symbolic-ref", "HEAD", "refs/heads/unlinked"]);
        assert_eq!(resolve_dir(&env.repo, &env.config).unwrap(), None);
        assert!(!env.config.cache_dir.join("arc.db").exists());
    }

    #[tokio::test]
    async fn corrupt_index_is_an_error_not_a_panic() {
        let Some(env) = env_on("linked") else { return };
        std::fs::create_dir_all(&env.config.cache_dir).unwrap();
        std::fs::write(env.config.cache_dir.join("arc.db"), "not a database").unwrap();
        let mut cache = cache_for(&env);
        assert!(
            current_context(&env.repo, &env.config, &mut cache)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn link_refuses_trunk_branches() {
        let Some(env) = env_on("main") else { return };
        let mut cache = cache_for(&env);
        cache.upsert_story(story_seven());
        let err = link_current_branch(&env.repo, &env.config, &mut cache, 7.into())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("trunk branch"), "{err}");

        run_git(&env.repo, &["symbolic-ref", "HEAD", "refs/heads/release"]);
        run_git(&env.repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
        run_git(
            &env.repo,
            &["update-ref", "refs/remotes/origin/release", "HEAD"],
        );
        run_git(
            &env.repo,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/release",
            ],
        );
        let err = link_current_branch(&env.repo, &env.config, &mut cache, 7.into())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("trunk branch"), "{err}");
        assert!(!env.config.notes_dir.join("stories/story-seven.md").exists());

        run_git(&env.repo, &["checkout", "-q", "-b", "feature"]);
        let (branch, _, outcome) =
            link_current_branch(&env.repo, &env.config, &mut cache, 7.into())
                .await
                .unwrap();
        assert_eq!((branch, outcome), (br("feature"), LinkOutcome::Added));
    }

    #[tokio::test]
    async fn rename_updates_the_linked_note() {
        let Some(env) = env_on("wip") else { return };
        run_git(&env.repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
        let mut cache = cache_for(&env);
        cache.upsert_story(story_seven());
        link_current_branch(&env.repo, &env.config, &mut cache, 7.into())
            .await
            .unwrap();
        let renamed = rename_to_canonical(&env.repo, &env.config, &mut cache, 7.into())
            .await
            .unwrap();
        let note = rel("stories/story-seven.md");
        assert!(
            matches!(renamed.note, NoteRename::Updated(ref r) if *r == note),
            "{:?}",
            renamed.note
        );
        let body = std::fs::read_to_string(note.abs(&env.config.notes_dir)).unwrap();
        assert_eq!(
            Frontmatter::from_note(&body).unwrap().branches,
            vec!["tenzin/sc-7/story-seven"]
        );
        let conn = arc_index::open(&env.config.cache_dir).unwrap();
        assert_eq!(
            arc_index::story_for_branch(&conn, &br("wip")).unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn rename_refuses_branches_of_other_stories() {
        let Some(env) = env_on("me/sc-12/x") else {
            return;
        };
        run_git(&env.repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
        let mut cache = cache_for(&env);
        cache.upsert_story(story_seven());
        let err = rename_to_canonical(&env.repo, &env.config, &mut cache, 7.into())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("already names story 12"), "{err}");

        write_note(
            &env,
            "stories/other.md",
            "---\nstory_id: sc-99\nbranches: [taken]\n---\n",
        );
        run_git(&env.repo, &["checkout", "-q", "-b", "taken"]);
        let err = rename_to_canonical(&env.repo, &env.config, &mut cache, 7.into())
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("already linked to story 99"),
            "{err}"
        );
        assert_eq!(git::head(&env.repo).unwrap(), GitHead::Branch(br("taken")));
    }

    fn stackable(name: &str) -> StackableBranch {
        StackableBranch::parse(LinkableBranch::parse(br(name), None).unwrap()).unwrap()
    }

    #[test]
    fn stacked_links_use_the_parent_note_or_cached_story() {
        let Some(env) = env_on("unused") else { return };
        let parent = ResolvedStory {
            id: 7.into(),
            branch: br("tenzin/sc-7/x"),
            via: Via::BranchId,
        };
        let stacked = stackable("me/stacked");
        let mut cache = cache_for(&env);
        assert_eq!(
            link_stacked_branch(&env.config, &cache, &parent, &stacked).unwrap(),
            StackLink::NoNote
        );
        cache.upsert_story(story_seven());
        let note = rel("stories/story-seven.md");
        assert_eq!(
            link_stacked_branch(&env.config, &cache, &parent, &stacked).unwrap(),
            StackLink::Linked(note.clone(), LinkOutcome::Added)
        );
        assert_eq!(
            link_stacked_branch(&env.config, &cache, &parent, &stacked).unwrap(),
            StackLink::Linked(note.clone(), LinkOutcome::AlreadyLinked)
        );
        let resolved = ResolvedStory {
            id: 7.into(),
            branch: br("me/stacked"),
            via: Via::LinkedNote(note.clone()),
        };
        assert_eq!(
            story_note_for(&resolved, &env.config).unwrap().unwrap().0,
            note
        );
        let other = stackable("me/sc-12/y");
        assert!(link_stacked_branch(&env.config, &cache, &parent, &other).is_err());
    }
}
