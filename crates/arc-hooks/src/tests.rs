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
use crate::activity;
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
    git_with(dir, args, None);
}

fn git_at(dir: &Path, at: i64, args: &[&str]) {
    git_with(dir, args, Some(at));
}

fn git_with(dir: &Path, args: &[&str], at: Option<i64>) {
    let mut command = Command::new("git");
    command
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
        .args(args);
    if let Some(at) = at {
        command.env("GIT_COMMITTER_DATE", format!("@{at} +0000"));
    }
    let out = command.output().unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn minutes_ago(minutes: i64) -> i64 {
    arc_core::time::now().timestamp() - minutes * 60
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
    git(&repo, &["config", "user.email", "t@t"]);
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
                cwd: self.repo.clone(),
                started_at,
                state,
                baseline: activity::snapshot(&self.repo, arc_core::time::now()).unwrap(),
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
async fn post_bash_links_specific_branches_stacked_on_a_story_branch() {
    let Some(env) = env_on("tenzin/sc-7/x") else {
        return;
    };
    env.write_note("stories/story-seven.md", SEVEN);
    let t = minutes_ago(5);
    git_at(&env.repo, t, &["checkout", "-q", "-b", "me/stacked"]);
    assert_eq!(
        context_of(env.bash("git checkout -b me/stacked").await).as_deref(),
        Some("arc: linked me/stacked to sc-7 (stacked on tenzin/sc-7/x)")
    );
    assert_eq!(
        branches(&env.note("stories/story-seven.md")),
        ["me/stacked"]
    );

    git_at(&env.repo, t + 10, &["switch", "-q", "-c", "me/deeper"]);
    assert_eq!(
        context_of(env.bash("git switch -c me/deeper").await).as_deref(),
        Some("arc: linked me/deeper to sc-7 (stacked on me/stacked)")
    );

    git_at(&env.repo, t + 20, &["branch", "me/side", "tenzin/sc-7/x"]);
    std::fs::create_dir_all(env.repo.join("sub")).unwrap();
    let wt = env.tmp.path().join("wt");
    git_at(
        &env.repo,
        t + 30,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "me/in-tree",
            wt.to_str().unwrap(),
        ],
    );
    let both = format!(
        "cd sub && git branch me/side tenzin/sc-7/x; git -C .. worktree add -b me/in-tree {}",
        wt.display()
    );
    assert_eq!(
        context_of(env.bash(&both).await).as_deref(),
        Some(
            "arc: linked me/side to sc-7 (stacked on tenzin/sc-7/x)\narc: linked me/in-tree to sc-7 (stacked on me/deeper)"
        )
    );
    assert_eq!(
        branches(&env.note("stories/story-seven.md")),
        ["me/stacked", "me/deeper", "me/side", "me/in-tree"]
    );
}

#[tokio::test]
async fn post_bash_only_suggests_linking_generic_names() {
    let Some(env) = env_on("tenzin/sc-7/x") else {
        return;
    };
    env.write_note("stories/story-seven.md", SEVEN);
    for (name, at) in [("review", 5), ("wip", 4), ("wt", 3)] {
        git_at(&env.repo, minutes_ago(at), &["branch", name]);
        let suggestion = context_of(env.bash(&format!("git branch {name}")).await).unwrap();
        assert!(
            suggestion.starts_with(&format!(
                "arc: {name} was created from tenzin/sc-7/x (sc-7). Generic branch names aren't linked automatically"
            )),
            "{suggestion}"
        );
        assert!(suggestion.contains("`arc link sc-7`"), "{suggestion}");
    }
    assert!(branches(&env.note("stories/story-seven.md")).is_empty());

    let wt = env.tmp.path().join("me-sc-7-wt");
    git(&env.repo, &["worktree", "add", "-q", wt.to_str().unwrap()]);
    assert_eq!(
        env.bash(&format!("git worktree add {}", wt.display()))
            .await,
        None
    );
}

#[tokio::test]
async fn post_bash_reads_the_parent_from_the_reflog_not_the_current_head() {
    let Some(env) = env_on("tenzin/sc-7/x") else {
        return;
    };
    env.write_note("stories/story-seven.md", SEVEN);
    env.write_note(
        "stories/story-nine.md",
        "---\nid: story-nine\nstory_id: sc-9\n---\n\n# Story nine\n",
    );
    let t = minutes_ago(5);
    git_at(&env.repo, t - 60, &["branch", "me/sc-9/y"]);
    git_at(&env.repo, t, &["branch", "me/wip-save"]);
    git_at(&env.repo, t + 5, &["checkout", "-q", "me/sc-9/y"]);
    assert_eq!(
        context_of(
            env.bash("git branch me/wip-save && git checkout me/sc-9/y")
                .await
        )
        .as_deref(),
        Some("arc: linked me/wip-save to sc-7 (stacked on tenzin/sc-7/x)")
    );
    assert!(branches(&env.note("stories/story-nine.md")).is_empty());

    git_at(&env.repo, t + 60, &["branch", "me/from-head", "HEAD"]);
    git_at(&env.repo, t + 65, &["checkout", "-q", "tenzin/sc-7/x"]);
    assert_eq!(
        context_of(
            env.bash("git branch me/from-head HEAD && git checkout tenzin/sc-7/x")
                .await
        )
        .as_deref(),
        Some("arc: linked me/from-head to sc-9 (stacked on me/sc-9/y)")
    );

    git_at(&env.repo, t + 90, &["branch", "me/same-second", "HEAD"]);
    git_at(&env.repo, t + 90, &["checkout", "-q", "me/sc-9/y"]);
    let nudge = context_of(
        env.bash("git branch me/same-second HEAD && git checkout me/sc-9/y")
            .await,
    )
    .unwrap();
    assert!(
        nudge.starts_with("arc: new branch me/same-second isn't linked"),
        "{nudge}"
    );
    assert_eq!(
        branches(&env.note("stories/story-nine.md")),
        ["me/from-head"]
    );

    git_at(&env.repo, t + 120, &["checkout", "-q", "-b", "me/heredoc"]);
    assert_eq!(
        env.bash("cat <<EOF > notes.md\ngit checkout -b me/heredoc\nEOF")
            .await,
        None
    );

    git(
        &env.repo,
        &["-c", "core.logAllRefUpdates=false", "branch", "me/unlogged"],
    );
    assert_eq!(env.bash("git branch me/unlogged").await, None);
    assert_eq!(
        branches(&env.note("stories/story-seven.md")),
        ["me/wip-save"]
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
    git(&env.repo, &["branch", "me/orphan-work"]);
    let missing = context_of(env.bash("git branch me/orphan-work").await).unwrap();
    assert!(
        missing.starts_with(
            "arc: me/orphan-work was created from tenzin/sc-7/x (sc-7), but sc-7 has no story note"
        ),
        "{missing}"
    );

    let mut cache = env.cache();
    cache.upsert_story(story_seven());
    git(&env.repo, &["branch", "me/cached"]);
    let linked = context_of(
        env.hook_with(
            Hook::PostBash,
            env.bash_input("git branch me/cached"),
            cache,
        )
        .await,
    );
    assert_eq!(
        linked.as_deref(),
        Some("arc: linked me/cached to sc-7 (stacked on tenzin/sc-7/x)")
    );
    assert_eq!(branches(&env.note("stories/story-seven.md")), ["me/cached"]);

    env.write_note(
        "stories/other.md",
        "---\nstory_id: sc-99\nbranches: [me/taken]\n---\n",
    );
    git(&env.repo, &["branch", "me/taken"]);
    assert_eq!(env.bash("git branch me/taken").await, None);
    git(&env.repo, &["branch", "me/sc-12/z"]);
    assert_eq!(env.bash("git branch me/sc-12/z").await, None);
    assert_eq!(branches(&env.note("stories/story-seven.md")), ["me/cached"]);
}

fn stamp(at: DateTime<FixedOffset>) -> String {
    at.format("%Y-%m-%d %H:%M %:z").to_string()
}

#[tokio::test]
async fn stop_asks_for_a_handoff_once_after_new_work() {
    let Some(env) = env_on("tenzin/sc-7/x") else {
        return;
    };
    let now = arc_core::time::now();
    assert_eq!(env.hook(Hook::Stop, env.event("Stop")).await, None);

    env.mark(now - TimeDelta::hours(1), MarkerState::Fresh);
    assert_eq!(env.hook(Hook::Stop, env.event("Stop")).await, None);

    git(
        &env.repo,
        &["commit", "-q", "--allow-empty", "-m", "feat: work"],
    );
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
    let later = stamp(now + TimeDelta::minutes(2));
    env.write_note(
        "stories/story-seven.md",
        &format!("{SEVEN}\n## Sessions\n\n### {later} · tenzin/sc-7/x\n**Done:** a\n"),
    );
    std::fs::write(env.repo.join("new.txt"), "x").unwrap();
    assert_eq!(env.hook(Hook::Stop, env.event("Stop")).await, None);

    env.write_note(
        "stories/story-seven.md",
        &format!("{SEVEN}\n## Sessions\n\n### {later} · tenzin/sc-7/x · [auto]\n**Uncommitted:** 1 file\n"),
    );
    assert!(env.hook(Hook::Stop, env.event("Stop")).await.is_some());
}

#[tokio::test]
async fn stop_ignores_pre_session_state_and_teammates() {
    let Some(env) = env_on("tenzin/sc-7/x") else {
        return;
    };
    std::fs::write(env.repo.join("old.txt"), "x").unwrap();
    std::fs::write(env.repo.join("tracked.txt"), "x").unwrap();
    git(&env.repo, &["add", "tracked.txt"]);
    git(&env.repo, &["commit", "-q", "-m", "add tracked"]);
    std::fs::remove_file(env.repo.join("tracked.txt")).unwrap();
    env.mark(
        arc_core::time::now() - TimeDelta::hours(1),
        MarkerState::Fresh,
    );
    assert_eq!(env.hook(Hook::Stop, env.event("Stop")).await, None);

    git(
        &env.repo,
        &[
            "-c",
            "user.email=mate@example.com",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "feat: theirs",
        ],
    );
    std::fs::create_dir_all(env.repo.join("ignored")).unwrap();
    assert_eq!(env.hook(Hook::Stop, env.event("Stop")).await, None);

    std::fs::write(env.repo.join("fresh.txt"), "x").unwrap();
    assert!(env.hook(Hook::Stop, env.event("Stop")).await.is_some());
}

#[tokio::test]
async fn stop_and_session_end_stay_silent_for_legacy_markers() {
    let Some(env) = env_on("tenzin/sc-7/x") else {
        return;
    };
    env.write_note("stories/story-seven.md", SEVEN);
    let legacy = format!(
        r#"{{"session_id": "s1", "story_id": 7, "cwd": {:?}, "branch": null, "started_at": "{}", "state": "fresh"}}"#,
        env.repo,
        (arc_core::time::now() - TimeDelta::hours(1)).to_rfc3339()
    );
    std::fs::create_dir_all(env.config.cache_dir.join("sessions")).unwrap();
    std::fs::write(env.config.cache_dir.join("sessions/s1.json"), legacy).unwrap();
    git(
        &env.repo,
        &["commit", "-q", "--allow-empty", "-m", "feat: work"],
    );
    std::fs::write(env.repo.join("wip.txt"), "x").unwrap();
    assert_eq!(env.hook(Hook::Stop, env.event("Stop")).await, None);
    assert_eq!(
        env.hook(Hook::SessionEnd, env.event("SessionEnd")).await,
        None
    );
    assert_eq!(env.note("stories/story-seven.md"), SEVEN);
    assert_eq!(env.markers().load(&id()).unwrap(), None);
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
async fn session_end_records_only_work_after_the_last_handoff() {
    let Some(env) = env_on("tenzin/sc-7/x") else {
        return;
    };
    let now = arc_core::time::now();
    let logged = format!(
        "{SEVEN}\n## Sessions\n\n### {} · tenzin/sc-7/x\n**Done:** a\n",
        stamp(now + TimeDelta::minutes(2))
    );
    env.write_note("stories/story-seven.md", &logged);
    env.mark(now - TimeDelta::hours(1), MarkerState::Fresh);
    git(
        &env.repo,
        &["commit", "-q", "--allow-empty", "-m", "feat: logged"],
    );
    env.hook(Hook::SessionEnd, env.event("SessionEnd")).await;
    assert_eq!(env.note("stories/story-seven.md"), logged);
    assert_eq!(env.markers().load(&id()).unwrap(), None);

    env.mark(now - TimeDelta::hours(1), MarkerState::Nudged);
    git(
        &env.repo,
        &[
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "feat: before handoff",
        ],
    );
    git_at(
        &env.repo,
        (now + TimeDelta::minutes(5)).timestamp(),
        &["commit", "-q", "--allow-empty", "-m", "feat: after handoff"],
    );
    env.hook(Hook::SessionEnd, env.event("SessionEnd")).await;
    let note = env.note("stories/story-seven.md");
    let sessions = arc_context::latest_sessions(&note, 5);
    assert_eq!(sessions.len(), 2, "{note}");
    assert_eq!(
        sessions[1].lines().skip(1).collect::<Vec<_>>(),
        ["**Commits:**", "- feat: after handoff"]
    );

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
    assert_eq!(marker.state, MarkerState::Fresh);
    let baseline = marker.baseline.unwrap();
    assert_eq!(baseline.root, env.repo.canonicalize().unwrap());
    assert!(baseline.head.is_some() && baseline.dirty.is_empty());

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
    assert!(
        env.markers()
            .load(&id())
            .unwrap()
            .unwrap()
            .baseline
            .is_some()
    );
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
