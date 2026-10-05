use std::path::{Path, PathBuf};
use std::process::Command;

use arc_core::Config;
use arc_notes::NoteRelPath;
use arc_notes::note::frontmatter::Frontmatter;
use arc_shortcut::Cache;
use arc_shortcut::api::story::Story;
use arc_shortcut::api::user::{CurrentUser, MemberId, MentionName};
use chrono::{DateTime, FixedOffset, TimeDelta};
use serde_json::{Value, json};

use super::*;
use crate::input::SessionId;
use crate::marker::{MarkerState, Markers, SessionMarker};

pub(crate) fn config(root: &Path) -> Config {
    Config {
        notes_dir: root.join("notes"),
        cache_dir: root.join("cache"),
        api_token: String::new(),
        editor: String::new(),
        repositories_directory: root.to_path_buf(),
        mux: Default::default(),
    }
}

struct Env {
    tmp: tempfile::TempDir,
    repo: PathBuf,
    config: Config,
}

fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn git(dir: &Path, args: &[&str]) {
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
    git(&repo, &["init", "-q"]);
    git(
        &repo,
        &["symbolic-ref", "HEAD", &format!("refs/heads/{branch}")],
    );
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
    let config = config(tmp.path());
    std::fs::create_dir_all(config.notes_dir.join("stories")).unwrap();
    Some(Env { tmp, repo, config })
}

const SEVEN: &str = "---\nid: story-seven\nstory_id: sc-7\n---\n\n# Story seven\n";

impl Env {
    fn write_note(&self, path: &str, content: &str) {
        std::fs::write(self.config.notes_dir.join(path), content).unwrap();
        let conn = arc_index::open(&self.config.cache_dir).unwrap();
        arc_index::reindex_note(&conn, &NoteRelPath::parse(path).unwrap(), content).unwrap();
    }

    fn note(&self, path: &str) -> String {
        std::fs::read_to_string(self.config.notes_dir.join(path)).unwrap()
    }

    fn hook_env(&self) -> HookEnv {
        HookEnv {
            config: self.config.clone(),
            now: arc_core::time::now(),
            home: Some(self.tmp.path().to_path_buf()),
        }
    }

    fn cache(&self) -> Cache {
        let mut cache = Cache::default().with_cache_dir(self.config.cache_dir.clone());
        cache.set_user(CurrentUser {
            id: MemberId::from(uuid::Uuid::nil()),
            mention_name: MentionName::parse("tenzin").unwrap(),
        });
        cache
    }

    async fn hook(&self, hook: Hook, input: Value) -> Option<String> {
        self.hook_with(hook, input, self.cache()).await
    }

    async fn hook_with(&self, hook: Hook, input: Value, cache: Cache) -> Option<String> {
        respond(hook, &input.to_string(), self.hook_env(), cache)
            .await
            .unwrap()
    }

    async fn bash(&self, command: &str) -> Option<String> {
        self.hook(Hook::PostBash, self.bash_input(command)).await
    }

    fn bash_input(&self, command: &str) -> Value {
        json!({
            "session_id": "s1",
            "hook_event_name": "PostToolUse",
            "cwd": self.repo,
            "tool_name": "Bash",
            "tool_input": {"command": command},
            "tool_response": {"stdout": "", "stderr": "", "interrupted": false}
        })
    }

    fn event(&self, event: &str) -> Value {
        json!({
            "session_id": "s1",
            "hook_event_name": event,
            "cwd": self.repo,
            "source": "startup",
            "reason": "other"
        })
    }

    fn markers(&self) -> Markers {
        Markers::in_cache(&self.config.cache_dir)
    }

    fn mark(&self, started_at: DateTime<FixedOffset>, state: MarkerState) {
        self.markers()
            .save(&SessionMarker {
                session_id: id(),
                story_id: Some(7.into()),
                cwd: self.repo.clone(),
                branch: None,
                started_at,
                state,
            })
            .unwrap();
    }
}

fn id() -> SessionId {
    SessionId::parse("s1").unwrap()
}

fn context_of(output: Option<String>) -> Option<String> {
    let value: Value = serde_json::from_str(&output?).unwrap();
    assert_eq!(value["hookSpecificOutput"]["hookEventName"], "PostToolUse");
    Some(
        value["hookSpecificOutput"]["additionalContext"]
            .as_str()?
            .to_owned(),
    )
}

fn branches(note: &str) -> Vec<String> {
    Frontmatter::from_note(note).unwrap().branches
}

fn story_seven() -> Story {
    Story {
        id: 7.into(),
        name: "Story seven".into(),
        app_url: "https://app.shortcut.com/x/story/7".into(),
        story_type: "feature".into(),
        ..Default::default()
    }
}

#[tokio::test]
async fn post_bash_links_branches_stacked_on_a_story_branch() {
    let Some(env) = env_on("tenzin/sc-7/x") else {
        return;
    };
    env.write_note("stories/story-seven.md", SEVEN);
    git(&env.repo, &["checkout", "-q", "-b", "stacked"]);
    assert_eq!(
        context_of(env.bash("git checkout -b stacked").await).as_deref(),
        Some("arc: linked stacked to sc-7 (stacked on tenzin/sc-7/x)")
    );
    assert_eq!(branches(&env.note("stories/story-seven.md")), ["stacked"]);

    git(&env.repo, &["switch", "-q", "-c", "deeper"]);
    assert_eq!(
        context_of(env.bash("git switch -c deeper").await).as_deref(),
        Some("arc: linked deeper to sc-7 (stacked on stacked)")
    );

    git(&env.repo, &["branch", "side", "tenzin/sc-7/x"]);
    std::fs::create_dir_all(env.repo.join("sub")).unwrap();
    let wt = env.tmp.path().join("wt");
    git(
        &env.repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "in-tree",
            wt.to_str().unwrap(),
        ],
    );
    let both = format!(
        "cd sub && git branch side tenzin/sc-7/x; git -C .. worktree add -b in-tree {}",
        wt.display()
    );
    assert_eq!(
        context_of(env.bash(&both).await).as_deref(),
        Some(
            "arc: linked side to sc-7 (stacked on tenzin/sc-7/x)\narc: linked in-tree to sc-7 (stacked on deeper)"
        )
    );
    assert_eq!(
        branches(&env.note("stories/story-seven.md")),
        ["stacked", "deeper", "side", "in-tree"]
    );
}

#[tokio::test]
async fn post_bash_nudges_unlinked_branches_and_skips_the_rest() {
    let Some(env) = env_on("main") else { return };
    git(&env.repo, &["checkout", "-q", "-b", "feature"]);
    let nudge = context_of(env.bash("git checkout -b feature").await).unwrap();
    assert!(
        nudge.starts_with("arc: new branch feature isn't linked"),
        "{nudge}"
    );
    assert!(nudge.contains("`arc link <id>`"), "{nudge}");

    assert_eq!(env.bash("git checkout -b never-created").await, None);
    assert_eq!(env.bash("git status && ls").await, None);
    let edit = json!({"session_id": "s1", "hook_event_name": "PostToolUse", "cwd": env.repo, "tool_name": "Edit", "tool_input": {}});
    assert_eq!(env.hook(Hook::PostBash, edit).await, None);

    git(&env.repo, &["checkout", "-q", "-b", "me/sc-12/y"]);
    assert_eq!(env.bash("git checkout -b me/sc-12/y").await, None);

    git(&env.repo, &["checkout", "-q", "-b", "develop"]);
    assert_eq!(env.bash("git checkout -b develop").await, None);
}

#[tokio::test]
async fn post_bash_respects_other_stories_and_missing_notes() {
    let Some(env) = env_on("tenzin/sc-7/x") else {
        return;
    };
    git(&env.repo, &["branch", "orphan-work"]);
    let missing = context_of(env.bash("git branch orphan-work").await).unwrap();
    assert!(
        missing.starts_with(
            "arc: orphan-work was created from tenzin/sc-7/x (sc-7), but sc-7 has no story note"
        ),
        "{missing}"
    );

    let mut cache = env.cache();
    cache.upsert_story(story_seven());
    git(&env.repo, &["branch", "cached"]);
    let linked = context_of(
        env.hook_with(Hook::PostBash, env.bash_input("git branch cached"), cache)
            .await,
    );
    assert_eq!(
        linked.as_deref(),
        Some("arc: linked cached to sc-7 (stacked on tenzin/sc-7/x)")
    );
    assert_eq!(branches(&env.note("stories/story-seven.md")), ["cached"]);

    env.write_note(
        "stories/other.md",
        "---\nstory_id: sc-99\nbranches: [taken]\n---\n",
    );
    git(&env.repo, &["branch", "taken"]);
    assert_eq!(env.bash("git branch taken").await, None);
    git(&env.repo, &["branch", "me/sc-12/z"]);
    assert_eq!(env.bash("git branch me/sc-12/z").await, None);
    assert_eq!(branches(&env.note("stories/story-seven.md")), ["cached"]);
}

#[tokio::test]
async fn stop_asks_for_a_handoff_once_after_new_work() {
    let Some(env) = env_on("tenzin/sc-7/x") else {
        return;
    };
    let now = arc_core::time::now();
    assert_eq!(env.hook(Hook::Stop, env.event("Stop")).await, None);

    env.mark(now + TimeDelta::hours(1), MarkerState::Fresh);
    assert_eq!(env.hook(Hook::Stop, env.event("Stop")).await, None);

    env.mark(now - TimeDelta::hours(1), MarkerState::Fresh);
    let blocked: Value =
        serde_json::from_str(&env.hook(Hook::Stop, env.event("Stop")).await.unwrap()).unwrap();
    assert_eq!(blocked["decision"], "block");
    let reason = blocked["reason"].as_str().unwrap();
    assert!(reason.contains("`log_session`"), "{reason}");
    assert!(
        reason.contains(&format!("cwd: \"{}\"", env.repo.display())),
        "{reason}"
    );
    assert_eq!(
        env.markers().load(&id()).unwrap().unwrap().state,
        MarkerState::Nudged
    );
    assert_eq!(env.hook(Hook::Stop, env.event("Stop")).await, None);

    env.mark(now - TimeDelta::hours(1), MarkerState::Fresh);
    let later = (now + TimeDelta::minutes(2)).format("%Y-%m-%d %H:%M %:z");
    env.write_note(
        "stories/story-seven.md",
        &format!("{SEVEN}\n## Sessions\n\n### {later} · tenzin/sc-7/x\n**Done:** a\n"),
    );
    assert_eq!(env.hook(Hook::Stop, env.event("Stop")).await, None);

    std::fs::write(env.repo.join("new.txt"), "x").unwrap();
    assert_eq!(env.hook(Hook::Stop, env.event("Stop")).await, None);
}

#[tokio::test]
async fn stop_counts_dirty_files_edited_after_start() {
    let Some(env) = env_on("tenzin/sc-7/x") else {
        return;
    };
    std::fs::write(env.repo.join("old.txt"), "x").unwrap();
    let old = std::fs::File::options()
        .write(true)
        .open(env.repo.join("old.txt"))
        .unwrap();
    old.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(7200))
        .unwrap();
    let out = Command::new("git")
        .args([
            "-C",
            env.repo.to_str().unwrap(),
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "--amend",
            "--allow-empty",
            "-m",
            "old",
        ])
        .env("GIT_COMMITTER_DATE", "2020-01-01T00:00:00Z")
        .output()
        .unwrap();
    assert!(out.status.success());
    env.mark(
        arc_core::time::now() - TimeDelta::hours(1),
        MarkerState::Fresh,
    );
    assert_eq!(env.hook(Hook::Stop, env.event("Stop")).await, None);
    std::fs::write(env.repo.join("fresh.txt"), "x").unwrap();
    assert!(env.hook(Hook::Stop, env.event("Stop")).await.is_some());
}

#[tokio::test]
async fn session_end_leaves_a_breadcrumb_without_a_handoff() {
    let Some(env) = env_on("tenzin/sc-7/x") else {
        return;
    };
    env.write_note("stories/story-seven.md", SEVEN);
    env.mark(
        arc_core::time::now() - TimeDelta::hours(1),
        MarkerState::Nudged,
    );
    git(
        &env.repo,
        &["commit", "-q", "--allow-empty", "-m", "feat: first"],
    );
    git(
        &env.repo,
        &["commit", "-q", "--allow-empty", "-m", "fix: second"],
    );
    std::fs::write(env.repo.join("wip.txt"), "x").unwrap();
    assert_eq!(
        env.hook(Hook::SessionEnd, env.event("SessionEnd")).await,
        None
    );
    let note = env.note("stories/story-seven.md");
    let sessions = arc_context::latest_sessions(&note, 5);
    assert_eq!(sessions.len(), 1, "{note}");
    let lines: Vec<&str> = sessions[0].lines().collect();
    assert!(lines[0].ends_with(" · tenzin/sc-7/x · [auto]"), "{note}");
    assert_eq!(
        lines[1..],
        [
            "**Commits:**",
            "- fix: second",
            "- feat: first",
            "- init",
            "**Uncommitted:** 1 untracked file"
        ]
    );
    assert_eq!(arc_context::latest_handoff_at(&note), None);
    assert!(arc_context::latest_session_at(&note).is_some());
    assert_eq!(env.markers().load(&id()).unwrap(), None);

    assert_eq!(
        env.hook(Hook::SessionEnd, env.event("SessionEnd")).await,
        None
    );
    assert_eq!(env.note("stories/story-seven.md"), note);
}

#[tokio::test]
async fn session_end_is_quiet_after_a_handoff_or_without_work() {
    let Some(env) = env_on("tenzin/sc-7/x") else {
        return;
    };
    let now = arc_core::time::now();
    let stamp = now.format("%Y-%m-%d %H:%M %:z");
    let logged = format!("{SEVEN}\n## Sessions\n\n### {stamp} · tenzin/sc-7/x\n**Done:** a\n");
    env.write_note("stories/story-seven.md", &logged);
    env.mark(now - TimeDelta::hours(1), MarkerState::Fresh);
    git(
        &env.repo,
        &["commit", "-q", "--allow-empty", "-m", "feat: logged"],
    );
    env.hook(Hook::SessionEnd, env.event("SessionEnd")).await;
    assert_eq!(env.note("stories/story-seven.md"), logged);
    assert_eq!(env.markers().load(&id()).unwrap(), None);

    env.write_note("stories/story-seven.md", SEVEN);
    env.mark(now + TimeDelta::hours(1), MarkerState::Fresh);
    env.hook(Hook::SessionEnd, env.event("SessionEnd")).await;
    assert_eq!(env.note("stories/story-seven.md"), SEVEN);
}

#[tokio::test]
async fn session_start_briefs_story_sessions_and_marks_every_session() {
    let Some(env) = env_on("main") else { return };
    assert_eq!(
        env.hook(Hook::SessionStart, env.event("SessionStart"))
            .await,
        None
    );
    let marker = env.markers().load(&id()).unwrap().unwrap();
    assert_eq!((marker.story_id, marker.state), (None, MarkerState::Fresh));

    git(
        &env.repo,
        &["checkout", "-q", "-b", "tenzin/sc-7/story-seven"],
    );
    env.write_note("stories/story-seven.md", SEVEN);
    let mut cache = env.cache();
    cache.upsert_story(story_seven());
    let briefing = env
        .hook_with(Hook::SessionStart, env.event("SessionStart"), cache)
        .await
        .unwrap();
    assert!(
        briefing.starts_with("Shortcut story sc-7: Story seven\n"),
        "{briefing}"
    );
    assert!(
        briefing.contains("\narc workflow for sc-7:\n"),
        "{briefing}"
    );
    assert!(
        briefing.contains("(canonical: tenzin/sc-7/story-seven)"),
        "{briefing}"
    );
    let marker = env.markers().load(&id()).unwrap().unwrap();
    assert_eq!(marker.story_id, Some(7.into()));
}

#[tokio::test]
async fn mismatched_events_are_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let env = HookEnv {
        config: config(tmp.path()),
        now: arc_core::time::now(),
        home: None,
    };
    let stop = json!({"session_id": "s", "hook_event_name": "Stop", "cwd": "/"}).to_string();
    assert!(
        respond(Hook::SessionEnd, &stop, env, Cache::default())
            .await
            .is_err()
    );
}
