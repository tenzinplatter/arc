use std::fmt;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, bail};

use crate::Hook;
use crate::json::{Json, Object};

const ARC_BIN: &str = "arc";
const MCP_NAME: &str = "arc";
const CLAUDE_BIN: &str = "claude";
const BACKUP_MARKER: &str = ".arc-bak-";
const BACKUP_STAMP: &str = "%Y%m%dT%H%M%S%.9fZ";
const BACKUPS_KEPT: usize = 5;
const SHELL_SAFE: &str = "/._-+@%,:=";
const FAIL_SAFE: &str = " 2>/dev/null || true";
const CARGO_TARGET: &str = "target";
const CARGO_MANIFEST: &str = "Cargo.toml";
const CACHEDIR_TAG: &str = "CACHEDIR.TAG";
const MCP_COMMAND_LABEL: &str = "Command:";

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
    format!(
        "{} hook {hook}{FAIL_SAFE}",
        shell_quote(&arc.to_string_lossy())
    )
}

pub fn our_hook(command: &str) -> Option<Hook> {
    let command = command.trim();
    let command = command.strip_suffix(FAIL_SAFE).unwrap_or(command);
    Hook::ALL.into_iter().find(|hook| {
        command
            .strip_suffix(&format!(" hook {hook}"))
            .map(shell_unquote)
            .is_some_and(|exe| {
                Path::new(&exe)
                    .file_name()
                    .is_some_and(|name| name == ARC_BIN)
            })
    })
}

fn is_cargo_target(dir: &Path) -> bool {
    let named_target = dir.file_name().is_some_and(|name| name == CARGO_TARGET)
        && dir
            .parent()
            .is_some_and(|parent| parent.join(CARGO_MANIFEST).is_file());
    named_target
        || std::fs::read_to_string(dir.join(CACHEDIR_TAG)).is_ok_and(|tag| tag.contains("cargo"))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArcExe {
    Installed(PathBuf),
    DevBuild(PathBuf),
}

impl ArcExe {
    pub fn classify(exe: PathBuf) -> Self {
        match exe.ancestors().skip(1).any(is_cargo_target) {
            true => ArcExe::DevBuild(exe),
            false => ArcExe::Installed(exe),
        }
    }

    pub fn on_path() -> Option<Self> {
        find_on_path(ARC_BIN)
            .and_then(|bin| bin.canonicalize().ok())
            .map(Self::classify)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DevBuilds {
    Refuse,
    Allow,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HookBinary {
    Running(PathBuf),
    OnPath { arc: PathBuf, dev_build: PathBuf },
    DevBuild(PathBuf),
}

impl HookBinary {
    pub fn choose(
        running: ArcExe,
        on_path: Option<ArcExe>,
        dev_builds: DevBuilds,
    ) -> anyhow::Result<Self> {
        match (running, on_path, dev_builds) {
            (ArcExe::Installed(arc), _, _) => Ok(HookBinary::Running(arc)),
            (ArcExe::DevBuild(dev_build), Some(ArcExe::Installed(arc)), _) => {
                Ok(HookBinary::OnPath { arc, dev_build })
            }
            (ArcExe::DevBuild(dev), _, DevBuilds::Allow) => Ok(HookBinary::DevBuild(dev)),
            (ArcExe::DevBuild(dev), _, DevBuilds::Refuse) => bail!(
                "{} is a cargo build under target/, which changes or disappears on the next build, and there is no installed `arc` on PATH to use instead.\nInstall arc (e.g. `cargo install --path .`) and run `arc hook install` from there, or pass --allow-dev-build to point the hooks at this build anyway.",
                dev.display()
            ),
        }
    }

    pub fn path(&self) -> &Path {
        match self {
            HookBinary::Running(arc)
            | HookBinary::OnPath { arc, .. }
            | HookBinary::DevBuild(arc) => arc,
        }
    }

    fn describe(&self, out: &mut dyn Write) -> std::io::Result<()> {
        match self {
            HookBinary::Running(_) => Ok(()),
            HookBinary::OnPath { arc, dev_build } => writeln!(
                out,
                "using the arc on PATH ({}) rather than the dev build {}",
                arc.display(),
                dev_build.display()
            ),
            HookBinary::DevBuild(arc) => writeln!(
                out,
                "warning: hooks will run the dev build {}; reinstall once arc is installed",
                arc.display()
            ),
        }
    }
}

fn registration_group(reg: &Registration, arc: &Path) -> Json {
    let hook = Json::object([
        ("type", "command".into()),
        ("command", hook_command(arc, reg.hook).into()),
        ("timeout", reg.timeout_secs.into()),
    ]);
    let mut group = Object::new();
    if let Some(matcher) = reg.matcher {
        group.insert("matcher".into(), matcher.into());
    }
    group.insert("hooks".into(), vec![hook].into());
    Json::Object(group)
}

fn is_ours(hook: &Json) -> bool {
    hook.get("command")
        .and_then(Json::as_str)
        .is_some_and(|command| our_hook(command).is_some())
}

fn strip_ours(groups: Vec<Json>) -> (Vec<Json>, Option<usize>) {
    let mut first = None;
    let mut kept = Vec::new();
    for mut group in groups {
        let Some(hooks) = group.get_mut("hooks").and_then(Json::as_array_mut) else {
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

fn hooks_object(settings: &mut Json) -> anyhow::Result<&mut Object> {
    let root = settings
        .as_object_mut()
        .context("settings.json is not a JSON object")?;
    root.entry("hooks".to_owned())
        .or_insert_with(Json::empty_object)
        .as_object_mut()
        .context("`hooks` in settings.json is not an object")
}

fn ensure_arrays(hooks: &Object) -> anyhow::Result<()> {
    match hooks.iter().find(|(_, groups)| !groups.is_array()) {
        Some((event, _)) => bail!("`hooks.{event}` in settings.json is not an array"),
        None => Ok(()),
    }
}

fn take_groups(groups: &mut Json) -> Vec<Json> {
    match groups.take() {
        Json::Array(groups) => groups,
        _ => Vec::new(),
    }
}

fn strip_events(hooks: &mut Object, skip: impl Fn(&str) -> bool) {
    for (event, groups) in hooks.iter_mut() {
        if skip(event) {
            continue;
        }
        let (kept, removed) = strip_ours(take_groups(groups));
        *groups = match removed.is_some() && kept.is_empty() {
            true => Json::Null,
            false => Json::Array(kept),
        };
    }
    hooks.retain(|_, groups| *groups != Json::Null);
}

pub fn with_hooks(mut settings: Json, arc: &Path) -> anyhow::Result<Json> {
    let hooks = hooks_object(&mut settings)?;
    ensure_arrays(hooks)?;
    strip_events(hooks, |event| {
        REGISTRATIONS.iter().any(|r| r.event == event)
    });
    for reg in &REGISTRATIONS {
        let groups = hooks
            .entry(reg.event.to_owned())
            .or_insert_with(|| Json::Array(Vec::new()));
        let (mut kept, first) = strip_ours(take_groups(groups));
        kept.insert(first.unwrap_or(kept.len()), registration_group(reg, arc));
        *groups = Json::Array(kept);
    }
    Ok(settings)
}

pub fn without_hooks(mut settings: Json) -> anyhow::Result<Json> {
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
    let events = hooks.len();
    strip_events(hooks, |_| false);
    if events > 0 && hooks.is_empty() {
        root.shift_remove("hooks");
    }
    Ok(settings)
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH")
        .iter()
        .flat_map(std::env::split_paths)
        .map(|dir| dir.join(name))
        .find(|bin| bin.is_file())
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
        find_on_path(CLAUDE_BIN).map_or(Claude::Missing, Claude::Cli)
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpRegistration {
    Missing,
    Current,
    Other(Option<String>),
}

impl McpRegistration {
    pub fn parse(listed: bool, details: &str, arc: &Path) -> Self {
        if !listed {
            return McpRegistration::Missing;
        }
        let command = details.lines().find_map(|line| {
            line.trim()
                .strip_prefix(MCP_COMMAND_LABEL)
                .map(|command| command.trim().to_owned())
        });
        match command {
            Some(command) if Path::new(&command) == arc => McpRegistration::Current,
            command => McpRegistration::Other(command),
        }
    }
}

fn combined_output(ran: &std::process::Output) -> String {
    let parts: Vec<String> = [&ran.stdout, &ran.stderr]
        .into_iter()
        .map(|bytes| String::from_utf8_lossy(bytes).trim().to_owned())
        .filter(|part| !part.is_empty())
        .collect();
    match parts.is_empty() {
        true => format!("{} with no output", ran.status),
        false => parts.join("\n"),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    DryRun,
    Apply,
}

pub struct Installer {
    pub settings: PathBuf,
    pub claude: Claude,
}

enum Settings {
    Missing,
    Found { json: Json, trailing_newline: bool },
}

impl Settings {
    fn json(&self) -> Json {
        match self {
            Settings::Missing => Json::empty_object(),
            Settings::Found { json, .. } => json.clone(),
        }
    }

    fn trailing_newline(&self) -> bool {
        match self {
            Settings::Missing => true,
            Settings::Found {
                trailing_newline, ..
            } => *trailing_newline,
        }
    }

    fn unchanged(&self, updated: &Json) -> bool {
        match self {
            Settings::Found { json, .. } => json == updated,
            Settings::Missing => *updated == Json::empty_object(),
        }
    }
}

fn read_settings(path: &Path) -> anyhow::Result<Settings> {
    match std::fs::read_to_string(path) {
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(Settings::Missing),
        Err(e) => Err(e).with_context(|| format!("read {}", path.display())),
        Ok(text) if text.trim().is_empty() => Ok(Settings::Found {
            json: Json::empty_object(),
            trailing_newline: true,
        }),
        Ok(text) => Ok(Settings::Found {
            json: serde_json::from_str(&text)
                .with_context(|| format!("{} is not valid JSON", path.display()))?,
            trailing_newline: text.ends_with('\n'),
        }),
    }
}

fn rendered(settings: &Json, trailing_newline: bool) -> anyhow::Result<String> {
    let text = serde_json::to_string_pretty(settings)?;
    Ok(match trailing_newline {
        true => format!("{text}\n"),
        false => text,
    })
}

fn backup_prefix(path: &Path) -> String {
    format!(
        "{}{BACKUP_MARKER}",
        path.file_name().unwrap_or_default().to_string_lossy()
    )
}

fn backups(path: &Path) -> Vec<PathBuf> {
    let prefix = backup_prefix(path);
    let Some(dir) = path.parent() else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
        .map(|entry| entry.path())
        .collect();
    found.sort();
    found
}

fn back_up(path: &Path) -> anyhow::Result<PathBuf> {
    let stamp = chrono::Utc::now().format(BACKUP_STAMP);
    let backup = path.with_file_name(format!("{}{stamp}", backup_prefix(path)));
    std::fs::copy(path, &backup).with_context(|| format!("back up {}", path.display()))?;
    let all = backups(path);
    for old in &all[..all.len().saturating_sub(BACKUPS_KEPT)] {
        let _ = std::fs::remove_file(old);
    }
    Ok(backup)
}

fn write_settings(path: &Path, text: &str) -> anyhow::Result<Option<PathBuf>> {
    match path.exists() {
        true => {
            let backup = back_up(path)?;
            arc_notes::write::replace(path, text)?;
            Ok(Some(backup))
        }
        false => {
            arc_notes::write::create(path, text)?;
            Ok(None)
        }
    }
}

impl Installer {
    fn update_settings(
        &self,
        mode: Mode,
        change: impl FnOnce(Json) -> anyhow::Result<Json>,
        out: &mut dyn Write,
    ) -> anyhow::Result<()> {
        let current = read_settings(&self.settings)?;
        let updated = change(current.json())?;
        let path = self.settings.display();
        if current.unchanged(&updated) {
            writeln!(out, "{path} is already up to date")?;
            return Ok(());
        }
        let text = rendered(&updated, current.trailing_newline())?;
        match mode {
            Mode::DryRun => {
                writeln!(out, "would write {path}:")?;
                out.write_all(text.as_bytes())?;
                if !text.ends_with('\n') {
                    writeln!(out)?;
                }
            }
            Mode::Apply => match write_settings(&self.settings, &text)? {
                Some(backup) => writeln!(out, "updated {path} (backup: {})", backup.display())?,
                None => writeln!(out, "updated {path}")?,
            },
        }
        Ok(())
    }

    fn mcp_registration(&self, bin: &Path, arc: &Path) -> anyhow::Result<McpRegistration> {
        let got = ClaudeCommand::get().run(bin)?;
        Ok(McpRegistration::parse(
            got.status.success(),
            &String::from_utf8_lossy(&got.stdout),
            arc,
        ))
    }

    fn run_claude(
        &self,
        command: &ClaudeCommand,
        mode: Mode,
        out: &mut dyn Write,
    ) -> anyhow::Result<bool> {
        match (&self.claude, mode) {
            (Claude::Missing, _) => {
                writeln!(out, "claude CLI not found; run this yourself:\n  {command}")?;
                Ok(false)
            }
            (Claude::Cli(_), Mode::DryRun) => {
                writeln!(out, "would run: {command}")?;
                Ok(true)
            }
            (Claude::Cli(bin), Mode::Apply) => {
                let ran = command.run(bin)?;
                match ran.status.success() {
                    true => writeln!(out, "ran: {command}")?,
                    false => writeln!(
                        out,
                        "`{command}` failed:\n{}\nrun it yourself once that's fixed",
                        combined_output(&ran)
                    )?,
                }
                Ok(ran.status.success())
            }
        }
    }

    pub fn install(&self, arc: &HookBinary, mode: Mode, out: &mut dyn Write) -> anyhow::Result<()> {
        arc.describe(out)?;
        let arc = arc.path();
        self.update_settings(mode, |s| with_hooks(s, arc), out)?;
        if let (Claude::Cli(bin), Mode::Apply) = (&self.claude, mode) {
            match self.mcp_registration(bin, arc)? {
                McpRegistration::Current => {
                    writeln!(out, "MCP server `{MCP_NAME}` is already registered")?;
                    return Ok(());
                }
                McpRegistration::Other(command) => {
                    let was = command.unwrap_or_else(|| "something else".to_owned());
                    writeln!(
                        out,
                        "MCP server `{MCP_NAME}` runs {was}; re-registering it with {}",
                        arc.display()
                    )?;
                    if !self.run_claude(&ClaudeCommand::remove(), mode, out)? {
                        return Ok(());
                    }
                }
                McpRegistration::Missing => {}
            }
        }
        self.run_claude(&ClaudeCommand::add(arc), mode, out)
            .map(|_| ())
    }

    pub fn uninstall(&self, mode: Mode, out: &mut dyn Write) -> anyhow::Result<()> {
        if let Settings::Found { .. } = read_settings(&self.settings)? {
            self.update_settings(mode, without_hooks, out)?;
        }
        self.run_claude(&ClaudeCommand::remove(), mode, out)
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    const ARC: &str = "/opt/bin/arc";

    fn j(value: Value) -> Json {
        serde_json::from_value(value).unwrap()
    }

    fn v(settings: &Json) -> Value {
        serde_json::to_value(settings).unwrap()
    }

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
        v(&with_hooks(j(user_settings()), Path::new(ARC)).unwrap())
    }

    fn cmd(hook: &str) -> String {
        format!("{ARC} hook {hook} 2>/dev/null || true")
    }

    #[test]
    fn commands_quote_paths_fail_safe_and_are_recognised() {
        assert_eq!(
            hook_command(Path::new(ARC), Hook::Stop),
            "/opt/bin/arc hook stop 2>/dev/null || true"
        );
        let spaced = hook_command(Path::new("/Users/me/My Tools/arc"), Hook::PostBash);
        assert_eq!(
            spaced,
            "'/Users/me/My Tools/arc' hook post-bash 2>/dev/null || true"
        );
        assert_eq!(our_hook(&spaced), Some(Hook::PostBash));
        let quoted = hook_command(Path::new("/tmp/it's/arc"), Hook::SessionEnd);
        assert_eq!(
            quoted,
            r"'/tmp/it'\''s/arc' hook session-end 2>/dev/null || true"
        );
        assert_eq!(our_hook(&quoted), Some(Hook::SessionEnd));
        assert_eq!(
            our_hook("/old/place/arc hook session-start"),
            Some(Hook::SessionStart)
        );
        assert_eq!(our_hook("'/a b/arc' hook stop"), Some(Hook::Stop));
        assert_eq!(our_hook("arc hook stop"), Some(Hook::Stop));
        assert_eq!(our_hook("/bin/notarc hook stop 2>/dev/null || true"), None);
        assert_eq!(our_hook("arc hook install"), None);
        assert_eq!(our_hook("arc hook stop || true"), None);
        assert_eq!(our_hook("prettier --write"), None);
    }

    #[cfg(unix)]
    #[test]
    fn fail_safe_commands_exit_zero_without_noise() {
        let tmp = tempfile::tempdir().unwrap();
        let script = hook_command(&tmp.path().join("gone/arc"), Hook::Stop);
        let out = Command::new("sh").arg("-c").arg(&script).output().unwrap();
        assert!(out.status.success(), "{out:?}");
        assert!(out.stdout.is_empty() && out.stderr.is_empty(), "{out:?}");
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
                {"matcher": "Bash", "hooks": [{"type": "command", "command": cmd("post-bash"), "timeout": 10}]}
            ])
        );
        assert_eq!(
            settings["hooks"]["Stop"],
            json!([
                user["hooks"]["Stop"][0].take(),
                {"hooks": [{"type": "command", "command": cmd("stop"), "timeout": 10}]}
            ])
        );
        assert_eq!(
            settings["hooks"]["SessionStart"],
            json!([{"matcher": "startup|resume|clear|compact|fork", "hooks": [{"type": "command", "command": cmd("session-start"), "timeout": 10}]}])
        );
        assert_eq!(
            settings["hooks"]["SessionEnd"],
            json!([{"hooks": [{"type": "command", "command": cmd("session-end"), "timeout": 2}]}])
        );
    }

    const ORDERED: &str = r#"{
    "statusLine": {"type": "command", "command": "status.sh"},
    "env": {"ZED": "1", "ALPHA": "2"},
    "hooks": {
        "Stop": [{"hooks": [{"type": "command", "command": "notify-send done"}]}],
        "Notification": [{"matcher": "", "hooks": [{"command": "say hi", "type": "command"}]}]
    },
    "model": "opus"
}"#;

    #[test]
    fn install_and_uninstall_keep_key_order() {
        let original: Json = serde_json::from_str(ORDERED).unwrap();
        let installed = with_hooks(original.clone(), Path::new(ARC)).unwrap();
        let text = rendered(&installed, true).unwrap();
        let at = |needle: &str| {
            text.find(needle)
                .unwrap_or_else(|| panic!("{needle}: {text}"))
        };
        assert!(at("\"statusLine\"") < at("\"env\""), "{text}");
        assert!(at("\"ZED\"") < at("\"ALPHA\""), "{text}");
        assert!(at("\"env\"") < at("\"hooks\"") && at("\"hooks\"") < at("\"model\""));
        assert!(
            at("\"Stop\"") < at("\"Notification\"")
                && at("\"Notification\"") < at("\"SessionStart\"")
        );
        assert!(text.contains("\"command\": \"say hi\","), "{text}");
        assert!(
            text.starts_with("{\n  \"statusLine\": {\n    \"type\""),
            "{text}"
        );
        let removed = without_hooks(installed).unwrap();
        assert_eq!(
            rendered(&removed, false).unwrap(),
            rendered(&original, false).unwrap()
        );
        assert_eq!(
            rendered(&original, false).unwrap(),
            serde_json::to_string_pretty(&serde_json::from_str::<Json>(ORDERED).unwrap()).unwrap()
        );
    }

    #[test]
    fn install_is_idempotent_and_updates_in_place() {
        let once = with_hooks(j(user_settings()), Path::new(ARC)).unwrap();
        assert_eq!(with_hooks(once.clone(), Path::new(ARC)).unwrap(), once);
        let moved = v(&with_hooks(once.clone(), Path::new("/new/arc")).unwrap());
        let once = v(&once);
        assert_eq!(
            moved["hooks"]["PostToolUse"][1]["hooks"][0]["command"],
            "/new/arc hook post-bash 2>/dev/null || true"
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
        shared["hooks"]["PreCompact"] = json!([{"hooks": [{"type": "command", "command": "/old/arc hook stop 2>/dev/null || true"}]}]);
        let fixed = v(&with_hooks(j(shared), Path::new(ARC)).unwrap());
        assert_eq!(
            fixed["hooks"]["Stop"],
            json!([
                {"hooks": [{"type": "command", "command": cmd("stop"), "timeout": 10}]},
                {"hooks": [{"type": "command", "command": "notify-send done"}]}
            ])
        );
        assert!(fixed["hooks"].get("PreCompact").is_none());
    }

    #[test]
    fn uninstall_removes_only_our_entries() {
        let removed = without_hooks(j(installed())).unwrap();
        assert_eq!(v(&removed), user_settings());
        assert_eq!(
            v(&without_hooks(j(json!({"a": 1}))).unwrap()),
            json!({"a": 1})
        );
        let only_ours = with_hooks(Json::empty_object(), Path::new(ARC)).unwrap();
        assert_eq!(v(&without_hooks(only_ours).unwrap()), json!({}));
        let empty_event = json!({"hooks": {"Stop": []}});
        assert_eq!(
            v(&without_hooks(j(empty_event.clone())).unwrap()),
            empty_event
        );
    }

    #[test]
    fn refuses_unexpected_shapes() {
        let with = |value: Value| with_hooks(j(value), Path::new(ARC));
        assert!(with(json!([])).is_err());
        assert!(with(json!({"hooks": []})).is_err());
        assert!(with(json!({"hooks": {"Stop": {}}})).is_err());
        assert!(with(json!({"hooks": {"Other": "x"}})).is_err());
        assert!(without_hooks(j(json!({"hooks": {"Stop": 1}}))).is_err());
        let odd_group = json!({"hooks": {"Stop": [{"matcher": "x"}]}});
        assert_eq!(
            v(&with(odd_group).unwrap())["hooks"]["Stop"][0],
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

    #[test]
    fn mcp_registrations_compare_the_command() {
        let arc = Path::new(ARC);
        let details = |command: &str| {
            format!(
                "arc:\n  Scope: User config\n  Type: stdio\n  Command: {command}\n  Args: mcp\n"
            )
        };
        assert_eq!(
            McpRegistration::parse(false, "", arc),
            McpRegistration::Missing
        );
        assert_eq!(
            McpRegistration::parse(true, &details(ARC), arc),
            McpRegistration::Current
        );
        assert_eq!(
            McpRegistration::parse(true, &details("/opt/bin/arc2"), arc),
            McpRegistration::Other(Some("/opt/bin/arc2".into()))
        );
        assert_eq!(
            McpRegistration::parse(true, "arc:\n  Type: http\n  URL: https://x\n", arc),
            McpRegistration::Other(None)
        );
    }

    fn dev_tree() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("Cargo.toml"), "").unwrap();
        let exe = tmp.path().join("target/debug/arc");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        (tmp, exe)
    }

    #[test]
    fn dev_builds_are_recognised_and_replaced_or_refused() {
        let (tmp, dev) = dev_tree();
        assert_eq!(ArcExe::classify(dev.clone()), ArcExe::DevBuild(dev.clone()));
        let custom = tmp.path().join("elsewhere/build");
        std::fs::create_dir_all(custom.join("release")).unwrap();
        std::fs::write(
            custom.join("CACHEDIR.TAG"),
            "Signature: 8a477f597d28d172789f06886806bc55\n# This file is a cache directory tag created by cargo.\n",
        )
        .unwrap();
        let custom_exe = custom.join("release/arc");
        assert!(matches!(ArcExe::classify(custom_exe), ArcExe::DevBuild(_)));
        let plain = tmp.path().join("bin/arc");
        assert_eq!(
            ArcExe::classify(plain.clone()),
            ArcExe::Installed(plain.clone())
        );
        let stray = tmp.path().join("target-ish/target/arc");
        assert!(matches!(ArcExe::classify(stray), ArcExe::Installed(_)));

        let installed = || Some(ArcExe::Installed(plain.clone()));
        assert_eq!(
            HookBinary::choose(ArcExe::Installed(plain.clone()), None, DevBuilds::Refuse).unwrap(),
            HookBinary::Running(plain.clone())
        );
        assert_eq!(
            HookBinary::choose(
                ArcExe::DevBuild(dev.clone()),
                installed(),
                DevBuilds::Refuse
            )
            .unwrap(),
            HookBinary::OnPath {
                arc: plain.clone(),
                dev_build: dev.clone()
            }
        );
        let refused = HookBinary::choose(
            ArcExe::DevBuild(dev.clone()),
            Some(ArcExe::DevBuild(dev.clone())),
            DevBuilds::Refuse,
        )
        .unwrap_err()
        .to_string();
        assert!(refused.contains("--allow-dev-build"), "{refused}");
        assert_eq!(
            HookBinary::choose(ArcExe::DevBuild(dev.clone()), None, DevBuilds::Allow).unwrap(),
            HookBinary::DevBuild(dev)
        );
    }

    fn installer(dir: &Path, claude: Claude) -> Installer {
        Installer {
            settings: dir.join(".claude/settings.json"),
            claude,
        }
    }

    fn arc() -> HookBinary {
        HookBinary::Running(PathBuf::from(ARC))
    }

    fn output(run: impl FnOnce(&mut Vec<u8>) -> anyhow::Result<()>) -> String {
        let mut out = Vec::new();
        run(&mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    fn pretty(value: &Value) -> String {
        format!("{}\n", serde_json::to_string_pretty(value).unwrap())
    }

    fn backups_of(inst: &Installer) -> Vec<PathBuf> {
        backups(&inst.settings)
    }

    #[test]
    fn dry_run_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let inst = installer(tmp.path(), Claude::Missing);
        std::fs::create_dir_all(tmp.path().join(".claude")).unwrap();
        let original = pretty(&user_settings());
        std::fs::write(&inst.settings, &original).unwrap();
        let printed = output(|out| inst.install(&arc(), Mode::DryRun, out));
        assert!(
            printed.starts_with(&format!("would write {}:\n{{", inst.settings.display())),
            "{printed}"
        );
        assert!(
            printed.contains(&format!("\"command\": \"{}\"", cmd("post-bash"))),
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
    fn fake_claude(dir: &Path, registered: Option<&str>) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let bin = dir.join("claude");
        let log = dir.join("claude.log");
        let get = match registered {
            Some(command) => {
                format!("printf 'arc:\\n  Command: {command}\\n  Args: mcp\\n'; exit 0")
            }
            None => "echo 'No MCP server named arc'; exit 1".to_owned(),
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
    fn install_writes_backs_up_and_registers_mcp() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = fake_claude(tmp.path(), None);
        let inst = installer(tmp.path(), Claude::Cli(bin));
        std::fs::create_dir_all(tmp.path().join(".claude")).unwrap();
        let original = pretty(&user_settings());
        std::fs::write(&inst.settings, &original).unwrap();

        let first = output(|out| inst.install(&arc(), Mode::Apply, out));
        assert!(first.contains("(backup: "), "{first}");
        assert!(
            first.contains(
                "ran: claude mcp add --scope user --transport stdio arc -- /opt/bin/arc mcp"
            ),
            "{first}"
        );
        let written = std::fs::read_to_string(&inst.settings).unwrap();
        assert_eq!(
            written,
            rendered(
                &with_hooks(serde_json::from_str(&original).unwrap(), Path::new(ARC)).unwrap(),
                true
            )
            .unwrap()
        );
        let made = backups_of(&inst);
        assert_eq!(made.len(), 1);
        assert_eq!(std::fs::read_to_string(&made[0]).unwrap(), original);

        let second = output(|out| inst.install(&arc(), Mode::Apply, out));
        assert!(second.contains("is already up to date"), "{second}");
        assert_eq!(std::fs::read_to_string(&inst.settings).unwrap(), written);
        assert_eq!(backups_of(&inst), made);

        output(|out| inst.uninstall(Mode::Apply, out));
        assert_eq!(std::fs::read_to_string(&inst.settings).unwrap(), original);
        assert_eq!(backups_of(&inst).len(), 2);
        let log = std::fs::read_to_string(tmp.path().join("claude.log")).unwrap();
        assert_eq!(
            log,
            "mcp get arc\nmcp add --scope user --transport stdio arc -- /opt/bin/arc mcp\nmcp get arc\nmcp add --scope user --transport stdio arc -- /opt/bin/arc mcp\nmcp remove --scope user arc\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn backups_are_capped_and_untouched_files_are_not_rewritten() {
        let tmp = tempfile::tempdir().unwrap();
        let inst = installer(tmp.path(), Claude::Missing);
        std::fs::create_dir_all(tmp.path().join(".claude")).unwrap();
        std::fs::write(&inst.settings, "{\"model\": \"opus\"}").unwrap();
        for round in 0..4 {
            output(|out| {
                inst.install(
                    &HookBinary::Running(PathBuf::from(format!("/r{round}/arc"))),
                    Mode::Apply,
                    out,
                )
            });
            output(|out| inst.uninstall(Mode::Apply, out));
        }
        assert_eq!(backups_of(&inst).len(), BACKUPS_KEPT);
        assert_eq!(
            std::fs::read_to_string(&inst.settings).unwrap(),
            "{\n  \"model\": \"opus\"\n}"
        );
        let before = std::fs::metadata(&inst.settings)
            .unwrap()
            .modified()
            .unwrap();
        let printed = output(|out| inst.uninstall(Mode::Apply, out));
        assert!(printed.contains("already up to date"), "{printed}");
        assert_eq!(
            std::fs::metadata(&inst.settings)
                .unwrap()
                .modified()
                .unwrap(),
            before
        );
        assert_eq!(backups_of(&inst).len(), BACKUPS_KEPT);
        assert!(!tmp.path().join(".claude/settings.json.bak").exists());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_settings_are_written_through() {
        let tmp = tempfile::tempdir().unwrap();
        let dotfiles = tmp.path().join("dotfiles");
        std::fs::create_dir_all(&dotfiles).unwrap();
        let real = dotfiles.join("claude-settings.json");
        std::fs::write(&real, "{\"model\": \"opus\"}\n").unwrap();
        let inst = installer(tmp.path(), Claude::Missing);
        std::fs::create_dir_all(tmp.path().join(".claude")).unwrap();
        std::os::unix::fs::symlink(&real, &inst.settings).unwrap();
        output(|out| inst.install(&arc(), Mode::Apply, out));
        assert!(
            std::fs::symlink_metadata(&inst.settings)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        let written = std::fs::read_to_string(&real).unwrap();
        assert!(written.contains(&cmd("stop")), "{written}");
        assert_eq!(std::fs::read_dir(&dotfiles).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn install_skips_registered_mcp_and_creates_missing_settings() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = fake_claude(tmp.path(), Some(ARC));
        let inst = installer(tmp.path(), Claude::Cli(bin));
        let printed = output(|out| inst.install(&arc(), Mode::Apply, out));
        assert!(printed.contains("already registered"), "{printed}");
        assert!(backups_of(&inst).is_empty());
        let written: Json =
            serde_json::from_str(&std::fs::read_to_string(&inst.settings).unwrap()).unwrap();
        assert_eq!(
            written,
            with_hooks(Json::empty_object(), Path::new(ARC)).unwrap()
        );
        let log = std::fs::read_to_string(tmp.path().join("claude.log")).unwrap();
        assert_eq!(log, "mcp get arc\n");
    }

    #[cfg(unix)]
    #[test]
    fn install_reregisters_mcp_pointing_elsewhere() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = fake_claude(tmp.path(), Some("/old/target/debug/arc"));
        let inst = installer(tmp.path(), Claude::Cli(bin));
        let printed = output(|out| inst.install(&arc(), Mode::Apply, out));
        assert!(
            printed.contains("runs /old/target/debug/arc; re-registering it with /opt/bin/arc"),
            "{printed}"
        );
        let log = std::fs::read_to_string(tmp.path().join("claude.log")).unwrap();
        assert_eq!(
            log,
            "mcp get arc\nmcp remove --scope user arc\nmcp add --scope user --transport stdio arc -- /opt/bin/arc mcp\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn failed_claude_commands_report_stdout_and_stderr() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("claude");
        std::fs::write(
            &bin,
            "#!/bin/sh\nif [ \"$2\" = get ]; then exit 1; fi\necho 'on stdout'\necho 'on stderr' >&2\nexit 3\n",
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let inst = installer(tmp.path(), Claude::Cli(bin));
        let printed = output(|out| inst.install(&arc(), Mode::Apply, out));
        assert!(
            printed.contains("failed:\non stdout\non stderr\nrun it yourself"),
            "{printed}"
        );
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
