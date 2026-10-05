use std::fmt;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, bail};
use serde_json::{Map, Value, json};

use crate::Hook;

const ARC_BIN: &str = "arc";
const MCP_NAME: &str = "arc";
const CLAUDE_BIN: &str = "claude";
const BACKUP_SUFFIX: &str = "bak";
const SHELL_SAFE: &str = "/._-+@%,:=";

#[derive(Clone, Copy, Debug)]
struct Registration {
    hook: Hook,
    event: &'static str,
    matcher: Option<&'static str>,
    timeout_secs: u32,
}

const REGISTRATIONS: [Registration; 4] = [
    Registration {
        hook: Hook::SessionStart,
        event: "SessionStart",
        matcher: Some("startup|resume|clear|compact|fork"),
        timeout_secs: 10,
    },
    Registration {
        hook: Hook::PostBash,
        event: "PostToolUse",
        matcher: Some("Bash"),
        timeout_secs: 10,
    },
    Registration {
        hook: Hook::Stop,
        event: "Stop",
        matcher: None,
        timeout_secs: 10,
    },
    Registration {
        hook: Hook::SessionEnd,
        event: "SessionEnd",
        matcher: None,
        timeout_secs: 2,
    },
];

fn shell_quote(word: &str) -> String {
    let safe = !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || SHELL_SAFE.contains(c));
    match safe {
        true => word.to_owned(),
        false => format!("'{}'", word.replace('\'', r"'\''")),
    }
}

fn shell_unquote(word: &str) -> String {
    match word.strip_prefix('\'').and_then(|w| w.strip_suffix('\'')) {
        Some(inner) => inner.replace(r"'\''", "'"),
        None => word.to_owned(),
    }
}

pub fn hook_command(arc: &Path, hook: Hook) -> String {
    format!("{} hook {hook}", shell_quote(&arc.to_string_lossy()))
}

pub fn our_hook(command: &str) -> Option<Hook> {
    Hook::ALL.into_iter().find(|hook| {
        command
            .trim()
            .strip_suffix(&format!(" hook {hook}"))
            .map(shell_unquote)
            .is_some_and(|exe| {
                Path::new(&exe)
                    .file_name()
                    .is_some_and(|name| name == ARC_BIN)
            })
    })
}

fn registration_group(reg: &Registration, arc: &Path) -> Value {
    let hook = json!({
        "type": "command",
        "command": hook_command(arc, reg.hook),
        "timeout": reg.timeout_secs,
    });
    let mut group = Map::new();
    if let Some(matcher) = reg.matcher {
        group.insert("matcher".into(), matcher.into());
    }
    group.insert("hooks".into(), json!([hook]));
    Value::Object(group)
}

fn is_ours(hook: &Value) -> bool {
    hook.get("command")
        .and_then(Value::as_str)
        .is_some_and(|command| our_hook(command).is_some())
}

fn strip_ours(groups: Vec<Value>) -> (Vec<Value>, Option<usize>) {
    let mut first = None;
    let mut kept = Vec::new();
    for mut group in groups {
        let Some(hooks) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
            kept.push(group);
            continue;
        };
        let before = hooks.len();
        hooks.retain(|hook| !is_ours(hook));
        let removed = hooks.len() < before;
        if removed {
            first.get_or_insert(kept.len());
        }
        if !(removed && hooks.is_empty()) {
            kept.push(group);
        }
    }
    (kept, first)
}

fn hooks_object(settings: &mut Value) -> anyhow::Result<&mut Map<String, Value>> {
    let root = settings
        .as_object_mut()
        .context("settings.json is not a JSON object")?;
    root.entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .context("`hooks` in settings.json is not an object")
}

fn ensure_arrays(hooks: &Map<String, Value>) -> anyhow::Result<()> {
    match hooks.iter().find(|(_, groups)| !groups.is_array()) {
        Some((event, _)) => bail!("`hooks.{event}` in settings.json is not an array"),
        None => Ok(()),
    }
}

fn take_groups(groups: &mut Value) -> Vec<Value> {
    match groups.take() {
        Value::Array(groups) => groups,
        _ => Vec::new(),
    }
}

fn strip_events(hooks: &mut Map<String, Value>, skip: impl Fn(&str) -> bool) {
    for (event, groups) in hooks.iter_mut() {
        if skip(event) {
            continue;
        }
        let (kept, removed) = strip_ours(take_groups(groups));
        *groups = match removed.is_some() && kept.is_empty() {
            true => Value::Null,
            false => Value::Array(kept),
        };
    }
    hooks.retain(|_, groups| !groups.is_null());
}

pub fn with_hooks(mut settings: Value, arc: &Path) -> anyhow::Result<Value> {
    let hooks = hooks_object(&mut settings)?;
    ensure_arrays(hooks)?;
    strip_events(hooks, |event| {
        REGISTRATIONS.iter().any(|r| r.event == event)
    });
    for reg in &REGISTRATIONS {
        let groups = hooks.entry(reg.event).or_insert_with(|| json!([]));
        let (mut kept, first) = strip_ours(take_groups(groups));
        kept.insert(first.unwrap_or(kept.len()), registration_group(reg, arc));
        *groups = Value::Array(kept);
    }
    Ok(settings)
}

pub fn without_hooks(mut settings: Value) -> anyhow::Result<Value> {
    let Some(root) = settings.as_object_mut() else {
        bail!("settings.json is not a JSON object");
    };
    let Some(hooks) = root.get_mut("hooks") else {
        return Ok(settings);
    };
    let hooks = hooks
        .as_object_mut()
        .context("`hooks` in settings.json is not an object")?;
    ensure_arrays(hooks)?;
    strip_events(hooks, |_| false);
    Ok(settings)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Claude {
    Cli(PathBuf),
    Missing,
}

impl Claude {
    pub fn locate(explicit: Option<PathBuf>) -> Self {
        if let Some(bin) = explicit {
            return match bin.is_file() {
                true => Claude::Cli(bin),
                false => Claude::Missing,
            };
        }
        std::env::var_os("PATH")
            .iter()
            .flat_map(std::env::split_paths)
            .map(|dir| dir.join(CLAUDE_BIN))
            .find(|bin| bin.is_file())
            .map_or(Claude::Missing, Claude::Cli)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaudeCommand(Vec<String>);

impl ClaudeCommand {
    pub fn add(arc: &Path) -> Self {
        let arc = arc.to_string_lossy();
        Self(
            [
                "mcp",
                "add",
                "--scope",
                "user",
                "--transport",
                "stdio",
                MCP_NAME,
                "--",
                &arc,
                "mcp",
            ]
            .map(str::to_owned)
            .to_vec(),
        )
    }

    pub fn remove() -> Self {
        Self(
            ["mcp", "remove", "--scope", "user", MCP_NAME]
                .map(str::to_owned)
                .to_vec(),
        )
    }

    fn get() -> Self {
        Self(["mcp", "get", MCP_NAME].map(str::to_owned).to_vec())
    }

    fn run(&self, bin: &Path) -> anyhow::Result<std::process::Output> {
        Command::new(bin)
            .args(&self.0)
            .output()
            .with_context(|| format!("failed to run {}", bin.display()))
    }
}

impl fmt::Display for ClaudeCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let words: Vec<String> = self.0.iter().map(|w| shell_quote(w)).collect();
        write!(f, "{CLAUDE_BIN} {}", words.join(" "))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    DryRun,
    Apply,
}

pub struct Installer {
    pub settings: PathBuf,
    pub arc: PathBuf,
    pub claude: Claude,
}

enum Settings {
    Missing,
    Found(Value),
}

impl Settings {
    fn value(&self) -> Value {
        match self {
            Settings::Missing => json!({}),
            Settings::Found(value) => value.clone(),
        }
    }
}

fn read_settings(path: &Path) -> anyhow::Result<Settings> {
    match std::fs::read_to_string(path) {
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(Settings::Missing),
        Err(e) => Err(e).with_context(|| format!("read {}", path.display())),
        Ok(text) if text.trim().is_empty() => Ok(Settings::Found(json!({}))),
        Ok(text) => Ok(Settings::Found(
            serde_json::from_str(&text)
                .with_context(|| format!("{} is not valid JSON", path.display()))?,
        )),
    }
}

fn rendered(settings: &Value) -> anyhow::Result<String> {
    Ok(format!("{}\n", serde_json::to_string_pretty(settings)?))
}

fn backup_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{BACKUP_SUFFIX}"));
    path.with_file_name(name)
}

fn write_settings(path: &Path, settings: &Value) -> anyhow::Result<Option<PathBuf>> {
    let dir = path
        .parent()
        .with_context(|| format!("{} has no parent directory", path.display()))?;
    std::fs::create_dir_all(dir)?;
    let backup = backup_path(path);
    let backed_up = match path.exists() && !backup.exists() {
        true => {
            std::fs::copy(path, &backup).with_context(|| format!("back up {}", path.display()))?;
            Some(backup)
        }
        false => None,
    };
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    if let Ok(meta) = std::fs::metadata(path) {
        tmp.as_file().set_permissions(meta.permissions())?;
    }
    tmp.write_all(rendered(settings)?.as_bytes())?;
    tmp.as_file().sync_all()?;
    tmp.persist(path)
        .with_context(|| format!("write {}", path.display()))?;
    Ok(backed_up)
}

impl Installer {
    fn update_settings(
        &self,
        mode: Mode,
        change: impl FnOnce(Value) -> anyhow::Result<Value>,
        out: &mut dyn Write,
    ) -> anyhow::Result<()> {
        let current = read_settings(&self.settings)?;
        let updated = change(current.value())?;
        let unchanged = match &current {
            Settings::Found(value) => *value == updated,
            Settings::Missing => updated == json!({}),
        };
        let path = self.settings.display();
        match (mode, unchanged) {
            (_, true) => writeln!(out, "{path} is already up to date")?,
            (Mode::DryRun, false) => {
                writeln!(out, "would write {path}:")?;
                out.write_all(rendered(&updated)?.as_bytes())?;
            }
            (Mode::Apply, false) => match write_settings(&self.settings, &updated)? {
                Some(backup) => writeln!(out, "updated {path} (backup: {})", backup.display())?,
                None => writeln!(out, "updated {path}")?,
            },
        }
        Ok(())
    }

    fn mcp_registered(&self, bin: &Path) -> anyhow::Result<bool> {
        let got = ClaudeCommand::get().run(bin)?;
        Ok(got.status.success()
            && String::from_utf8_lossy(&got.stdout).contains(&*self.arc.to_string_lossy()))
    }

    fn run_claude(
        &self,
        command: &ClaudeCommand,
        mode: Mode,
        out: &mut dyn Write,
    ) -> anyhow::Result<()> {
        match (&self.claude, mode) {
            (Claude::Missing, _) => {
                writeln!(out, "claude CLI not found; run this yourself:\n  {command}")?
            }
            (Claude::Cli(_), Mode::DryRun) => writeln!(out, "would run: {command}")?,
            (Claude::Cli(bin), Mode::Apply) => {
                let ran = command.run(bin)?;
                match ran.status.success() {
                    true => writeln!(out, "ran: {command}")?,
                    false => writeln!(
                        out,
                        "`{command}` failed: {}\nrun it yourself once that's fixed",
                        String::from_utf8_lossy(&ran.stderr).trim()
                    )?,
                }
            }
        }
        Ok(())
    }

    pub fn install(&self, mode: Mode, out: &mut dyn Write) -> anyhow::Result<()> {
        self.update_settings(mode, |s| with_hooks(s, &self.arc), out)?;
        if let (Claude::Cli(bin), Mode::Apply) = (&self.claude, mode)
            && self.mcp_registered(bin)?
        {
            writeln!(out, "MCP server `{MCP_NAME}` is already registered")?;
            return Ok(());
        }
        self.run_claude(&ClaudeCommand::add(&self.arc), mode, out)
    }

    pub fn uninstall(&self, mode: Mode, out: &mut dyn Write) -> anyhow::Result<()> {
        if let Settings::Found(_) = read_settings(&self.settings)? {
            self.update_settings(mode, without_hooks, out)?;
        }
        self.run_claude(&ClaudeCommand::remove(), mode, out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ARC: &str = "/opt/bin/arc";

    fn user_settings() -> Value {
        json!({
            "model": "opus",
            "permissions": {"allow": ["Bash(git status)"]},
            "hooks": {
                "PostToolUse": [
                    {"matcher": "Edit|Write", "hooks": [{"type": "command", "command": "prettier --write"}]}
                ],
                "Stop": [
                    {"hooks": [{"type": "command", "command": "notify-send done"}]}
                ],
                "Notification": [
                    {"hooks": [{"type": "command", "command": "say hi"}]}
                ]
            },
            "statusLine": {"type": "command", "command": "status.sh"}
        })
    }

    fn installed() -> Value {
        with_hooks(user_settings(), Path::new(ARC)).unwrap()
    }

    #[test]
    fn commands_quote_paths_and_are_recognised() {
        assert_eq!(
            hook_command(Path::new(ARC), Hook::Stop),
            "/opt/bin/arc hook stop"
        );
        let spaced = hook_command(Path::new("/Users/me/My Tools/arc"), Hook::PostBash);
        assert_eq!(spaced, "'/Users/me/My Tools/arc' hook post-bash");
        assert_eq!(our_hook(&spaced), Some(Hook::PostBash));
        let quoted = hook_command(Path::new("/tmp/it's/arc"), Hook::SessionEnd);
        assert_eq!(our_hook(&quoted), Some(Hook::SessionEnd));
        assert_eq!(
            our_hook("/old/place/arc hook session-start"),
            Some(Hook::SessionStart)
        );
        assert_eq!(our_hook("arc hook stop"), Some(Hook::Stop));
        assert_eq!(our_hook("/bin/notarc hook stop"), None);
        assert_eq!(our_hook("arc hook install"), None);
        assert_eq!(our_hook("prettier --write"), None);
    }

    #[test]
    fn install_adds_entries_and_keeps_everything_else() {
        let settings = installed();
        let mut user = user_settings();
        assert_eq!(settings["model"], user["model"]);
        assert_eq!(settings["permissions"], user["permissions"]);
        assert_eq!(settings["statusLine"], user["statusLine"]);
        assert_eq!(
            settings["hooks"]["Notification"],
            user["hooks"]["Notification"]
        );
        assert_eq!(
            settings["hooks"]["PostToolUse"],
            json!([
                user["hooks"]["PostToolUse"][0].take(),
                {"matcher": "Bash", "hooks": [{"type": "command", "command": "/opt/bin/arc hook post-bash", "timeout": 10}]}
            ])
        );
        assert_eq!(
            settings["hooks"]["Stop"],
            json!([
                user["hooks"]["Stop"][0].take(),
                {"hooks": [{"type": "command", "command": "/opt/bin/arc hook stop", "timeout": 10}]}
            ])
        );
        assert_eq!(
            settings["hooks"]["SessionStart"],
            json!([{"matcher": "startup|resume|clear|compact|fork", "hooks": [{"type": "command", "command": "/opt/bin/arc hook session-start", "timeout": 10}]}])
        );
        assert_eq!(
            settings["hooks"]["SessionEnd"],
            json!([{"hooks": [{"type": "command", "command": "/opt/bin/arc hook session-end", "timeout": 2}]}])
        );
        let keys: Vec<&String> = settings.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["hooks", "model", "permissions", "statusLine"]);
    }

    #[test]
    fn install_is_idempotent_and_updates_in_place() {
        let once = installed();
        assert_eq!(with_hooks(once.clone(), Path::new(ARC)).unwrap(), once);
        let moved = with_hooks(once.clone(), Path::new("/new/arc")).unwrap();
        assert_eq!(
            moved["hooks"]["PostToolUse"][1]["hooks"][0]["command"],
            "/new/arc hook post-bash"
        );
        assert_eq!(moved["hooks"]["PostToolUse"].as_array().unwrap().len(), 2);
        assert_eq!(
            moved["hooks"]["PostToolUse"][0],
            once["hooks"]["PostToolUse"][0]
        );

        let mut shared = user_settings();
        shared["hooks"]["Stop"][0]["hooks"]
            .as_array_mut()
            .unwrap()
            .push(json!({"type": "command", "command": "/old/arc hook stop"}));
        shared["hooks"]["PreCompact"] =
            json!([{"hooks": [{"type": "command", "command": "/old/arc hook stop"}]}]);
        let fixed = with_hooks(shared, Path::new(ARC)).unwrap();
        assert_eq!(
            fixed["hooks"]["Stop"],
            json!([
                {"hooks": [{"type": "command", "command": "/opt/bin/arc hook stop", "timeout": 10}]},
                {"hooks": [{"type": "command", "command": "notify-send done"}]}
            ])
        );
        assert!(fixed["hooks"].get("PreCompact").is_none());
    }

    #[test]
    fn uninstall_removes_only_our_entries() {
        let removed = without_hooks(installed()).unwrap();
        assert_eq!(
            rendered(&removed).unwrap(),
            rendered(&user_settings()).unwrap()
        );
        assert_eq!(without_hooks(json!({"a": 1})).unwrap(), json!({"a": 1}));
        let only_ours = with_hooks(json!({}), Path::new(ARC)).unwrap();
        assert_eq!(without_hooks(only_ours).unwrap(), json!({"hooks": {}}));
        let empty_event = json!({"hooks": {"Stop": []}});
        assert_eq!(without_hooks(empty_event.clone()).unwrap(), empty_event);
    }

    #[test]
    fn refuses_unexpected_shapes() {
        assert!(with_hooks(json!([]), Path::new(ARC)).is_err());
        assert!(with_hooks(json!({"hooks": []}), Path::new(ARC)).is_err());
        assert!(with_hooks(json!({"hooks": {"Stop": {}}}), Path::new(ARC)).is_err());
        assert!(with_hooks(json!({"hooks": {"Other": "x"}}), Path::new(ARC)).is_err());
        assert!(without_hooks(json!({"hooks": {"Stop": 1}})).is_err());
        let odd_group = json!({"hooks": {"Stop": [{"matcher": "x"}]}});
        assert_eq!(
            with_hooks(odd_group, Path::new(ARC)).unwrap()["hooks"]["Stop"][0],
            json!({"matcher": "x"})
        );
    }

    #[test]
    fn mcp_commands_render_for_a_shell() {
        assert_eq!(
            ClaudeCommand::add(Path::new("/My Tools/arc")).to_string(),
            "claude mcp add --scope user --transport stdio arc -- '/My Tools/arc' mcp"
        );
        assert_eq!(
            ClaudeCommand::remove().to_string(),
            "claude mcp remove --scope user arc"
        );
    }

    fn installer(dir: &Path, claude: Claude) -> Installer {
        Installer {
            settings: dir.join(".claude/settings.json"),
            arc: PathBuf::from(ARC),
            claude,
        }
    }

    fn output(run: impl FnOnce(&mut Vec<u8>) -> anyhow::Result<()>) -> String {
        let mut out = Vec::new();
        run(&mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn dry_run_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let inst = installer(tmp.path(), Claude::Missing);
        std::fs::create_dir_all(tmp.path().join(".claude")).unwrap();
        let original = rendered(&user_settings()).unwrap();
        std::fs::write(&inst.settings, &original).unwrap();
        let printed = output(|out| inst.install(Mode::DryRun, out));
        assert!(
            printed.starts_with(&format!("would write {}:\n{{", inst.settings.display())),
            "{printed}"
        );
        assert!(
            printed.contains("\"command\": \"/opt/bin/arc hook post-bash\""),
            "{printed}"
        );
        assert!(printed.ends_with("claude CLI not found; run this yourself:\n  claude mcp add --scope user --transport stdio arc -- /opt/bin/arc mcp\n"), "{printed}");
        assert_eq!(std::fs::read_to_string(&inst.settings).unwrap(), original);
        assert_eq!(
            std::fs::read_dir(tmp.path().join(".claude"))
                .unwrap()
                .count(),
            1
        );
    }

    #[cfg(unix)]
    fn fake_claude(dir: &Path, registered: bool) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let bin = dir.join("claude");
        let log = dir.join("claude.log");
        let get = match registered {
            true => format!("echo 'arc: {ARC} mcp'; exit 0"),
            false => "exit 1".to_owned(),
        };
        let script = format!(
            "#!/bin/sh\necho \"$@\" >> '{}'\nif [ \"$2\" = get ]; then {get}; fi\nexit 0\n",
            log.display()
        );
        std::fs::write(&bin, script).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        bin
    }

    #[cfg(unix)]
    #[test]
    fn install_writes_once_backs_up_once_and_registers_mcp() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = fake_claude(tmp.path(), false);
        let inst = installer(tmp.path(), Claude::Cli(bin));
        std::fs::create_dir_all(tmp.path().join(".claude")).unwrap();
        let original = rendered(&user_settings()).unwrap();
        std::fs::write(&inst.settings, &original).unwrap();

        let first = output(|out| inst.install(Mode::Apply, out));
        assert!(first.contains("(backup: "), "{first}");
        assert!(
            first.contains(
                "ran: claude mcp add --scope user --transport stdio arc -- /opt/bin/arc mcp"
            ),
            "{first}"
        );
        let written = std::fs::read_to_string(&inst.settings).unwrap();
        assert_eq!(written, rendered(&installed()).unwrap());
        let backup = backup_path(&inst.settings);
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), original);

        let second = output(|out| inst.install(Mode::Apply, out));
        assert!(second.contains("is already up to date"), "{second}");
        assert_eq!(std::fs::read_to_string(&inst.settings).unwrap(), written);

        output(|out| inst.uninstall(Mode::Apply, out));
        assert_eq!(std::fs::read_to_string(&inst.settings).unwrap(), original);
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), original);
        let log = std::fs::read_to_string(tmp.path().join("claude.log")).unwrap();
        assert_eq!(
            log,
            "mcp get arc\nmcp add --scope user --transport stdio arc -- /opt/bin/arc mcp\nmcp get arc\nmcp add --scope user --transport stdio arc -- /opt/bin/arc mcp\nmcp remove --scope user arc\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn install_skips_registered_mcp_and_creates_missing_settings() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = fake_claude(tmp.path(), true);
        let inst = installer(tmp.path(), Claude::Cli(bin));
        let printed = output(|out| inst.install(Mode::Apply, out));
        assert!(printed.contains("already registered"), "{printed}");
        assert!(!backup_path(&inst.settings).exists());
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(&inst.settings).unwrap()).unwrap();
        assert_eq!(written, with_hooks(json!({}), Path::new(ARC)).unwrap());
        let log = std::fs::read_to_string(tmp.path().join("claude.log")).unwrap();
        assert_eq!(log, "mcp get arc\n");
    }

    #[test]
    fn locating_claude_honours_an_explicit_path() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(
            Claude::locate(Some(tmp.path().join("nope"))),
            Claude::Missing
        );
        let bin = tmp.path().join("claude");
        std::fs::write(&bin, "").unwrap();
        assert_eq!(Claude::locate(Some(bin.clone())), Claude::Cli(bin));
    }
}
