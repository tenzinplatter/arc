use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};

const ARC: &str = env!("CARGO_BIN_EXE_arc");

struct Home {
    dir: tempfile::TempDir,
}

impl Home {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn with_config(self) -> Self {
        let config_dir = self.path().join("config/arc");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(
            config_dir.join("config.toml"),
            format!(
                "notes_dir = {:?}\ncache_dir = {:?}\napi_token = \"token\"\neditor = \"vi\"\n",
                self.path().join("notes"),
                self.path().join("cache"),
            ),
        )
        .unwrap();
        std::fs::create_dir_all(self.path().join("notes")).unwrap();
        self
    }

    fn arc(&self, args: &[&str], stdin: &str) -> Output {
        self.arc_with_path(args, stdin, &std::env::var_os("PATH").unwrap_or_default())
    }

    fn arc_with_path(&self, args: &[&str], stdin: &str, path: &std::ffi::OsStr) -> Output {
        let mut child = Command::new(ARC)
            .args(args)
            .env_clear()
            .env("PATH", path)
            .env("HOME", self.path())
            .env("XDG_CONFIG_HOME", self.path().join("config"))
            .env("XDG_CACHE_HOME", self.path().join("cache-home"))
            .current_dir(self.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }
}

fn fixtures(cwd: &Path) -> Vec<(&'static str, Value)> {
    vec![
        (
            "session-start",
            json!({"session_id": "e2e", "hook_event_name": "SessionStart", "cwd": cwd, "source": "startup", "transcript_path": "/x.jsonl"}),
        ),
        (
            "post-bash",
            json!({"session_id": "e2e", "hook_event_name": "PostToolUse", "cwd": cwd, "tool_name": "Bash", "tool_input": {"command": "git status"}, "tool_response": {"stdout": "", "stderr": ""}}),
        ),
        (
            "stop",
            json!({"session_id": "e2e", "hook_event_name": "Stop", "cwd": cwd, "last_assistant_message": "done"}),
        ),
        (
            "session-end",
            json!({"session_id": "e2e", "hook_event_name": "SessionEnd", "cwd": cwd, "reason": "other"}),
        ),
    ]
}

fn assert_silent(out: &Output, what: &str) {
    assert!(out.status.success(), "{what}: {:?}", out.status);
    assert!(
        out.stdout.is_empty(),
        "{what}: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        out.stderr.is_empty(),
        "{what}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn hooks_are_silent_without_config() {
    let home = Home::new();
    for (hook, input) in fixtures(home.path()) {
        assert_silent(&home.arc(&["hook", hook], &input.to_string()), hook);
    }
    assert_silent(&home.arc(&["hook", "stop"], "not json"), "garbage stdin");
    assert_silent(&home.arc(&["hook", "stop"], ""), "empty stdin");
}

#[test]
fn hooks_are_silent_when_no_story_resolves() {
    let home = Home::new().with_config();
    let repo = home.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let git = Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(&repo)
        .output();
    let dirs = match git {
        Ok(out) if out.status.success() => vec![home.path().to_path_buf(), repo],
        _ => vec![home.path().to_path_buf()],
    };
    let marker = home.path().join("cache/sessions/e2e.json");
    for dir in dirs {
        for (hook, input) in fixtures(&dir) {
            assert_silent(&home.arc(&["hook", hook], &input.to_string()), hook);
            assert_eq!(marker.exists(), hook != "session-end", "{hook}");
        }
    }
    assert_silent(
        &home.arc(
            &["hook", "session-end"],
            &fixtures(home.path())[0].1.to_string(),
        ),
        "mismatched event",
    );
}

fn dry_run(home: &Home, path: &Path, extra: &[&str]) -> Output {
    let missing_claude = home.path().join("no-claude");
    let mut args = vec![
        "hook",
        "install",
        "--dry-run",
        "--claude",
        missing_claude.to_str().unwrap(),
    ];
    args.extend(extra);
    home.arc_with_path(&args, "", path.as_os_str())
}

fn assert_dry_run_installs(out: Output, home: &Home, arc: &Path) -> String {
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let printed = String::from_utf8(out.stdout).unwrap();
    let settings = home.path().join(".claude/settings.json");
    assert!(
        printed.contains(&format!("would write {}:\n", settings.display())),
        "{printed}"
    );
    assert!(
        printed.contains(&format!(
            "\"command\": \"{} hook post-bash 2>/dev/null || true\"",
            arc.display()
        )),
        "{printed}"
    );
    assert!(
        printed.contains(&format!(
            "claude mcp add --scope user --transport stdio arc -- {} mcp",
            arc.display()
        )),
        "{printed}"
    );
    assert!(!home.path().join(".claude").exists());
    printed
}

#[cfg(unix)]
#[test]
fn install_prefers_an_installed_arc_over_a_dev_build() {
    use std::os::unix::fs::PermissionsExt;
    let home = Home::new();
    let bin = home.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let installed = bin.join("arc");
    std::fs::write(&installed, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&installed, std::fs::Permissions::from_mode(0o755)).unwrap();
    let installed = std::fs::canonicalize(installed).unwrap();
    let printed = assert_dry_run_installs(dry_run(&home, &bin, &[]), &home, &installed);
    assert!(
        printed.starts_with(&format!(
            "using the arc on PATH ({}) rather than the dev build ",
            installed.display()
        )),
        "{printed}"
    );
}

#[test]
fn install_refuses_a_dev_build_unless_allowed() {
    let home = Home::new();
    let empty = home.path().join("empty-bin");
    std::fs::create_dir_all(&empty).unwrap();
    let refused = dry_run(&home, &empty, &[]);
    assert!(!refused.status.success());
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(stderr.contains("--allow-dev-build"), "{stderr}");
    assert!(refused.stdout.is_empty());
    assert!(!home.path().join(".claude").exists());

    let exe = std::fs::canonicalize(ARC).unwrap();
    let printed =
        assert_dry_run_installs(dry_run(&home, &empty, &["--allow-dev-build"]), &home, &exe);
    assert!(
        printed.starts_with("warning: hooks will run the dev build "),
        "{printed}"
    );
}
