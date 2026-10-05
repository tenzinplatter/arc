use std::path::PathBuf;

use arc_context::EntryText;
use arc_core::StoryId;
use arc_hooks::install::Mode;
use clap::builder::{BoolValueParser, TypedValueParser};
use clap::{ArgAction, Args, Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(name = "arc")]
#[command(about = "Note manager with shortcut integration", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    #[command(alias = "n")]
    Note,

    #[command(alias = "t")]
    Tmux,

    #[command(alias = "clear")]
    ClearCache,

    #[command(alias = "o")]
    Open,

    #[command()]
    Cat,

    /// Set the active story by its Shortcut id (fetched from Shortcut if not cached).
    #[command(alias = "a")]
    Active { id: StoryId },

    /// Migrate config, cache, and notes directories from the old `shortcut-notes` name to `arc`.
    #[command()]
    Migrate,

    /// Background daemon that parses todos out of notes.
    #[command(subcommand)]
    Daemon(DaemonCommands),

    /// MCP server (stdio) for LLM clients: notes index, note/todo writes and cached Shortcut stories.
    #[command()]
    Mcp,

    /// Print the Shortcut story for the current git branch.
    ///
    /// Shows its name, state, URL, canonical branch, note path, recent `## Sessions` entries and
    /// the note's open todos. The story comes from an
    /// `sc-<id>` in the branch name, else from a story note listing the branch under `branches:`.
    /// Prints nothing when no story resolves. Only hits Shortcut when the story isn't cached.
    #[command()]
    Context {
        /// Output format.
        #[arg(long, value_enum, default_value_t = ContextFormat::Text)]
        format: ContextFormat,
    },

    /// Print the canonical branch name for a story: `{mention_name}/sc-{id}/{slug}[-{suffix}]`.
    #[command()]
    Branch {
        /// Shortcut story id (`123` or `sc-123`). Defaults to the story resolved from the current git branch.
        id: Option<StoryId>,
        /// Slugified and appended to the story slug as `-{suffix}`.
        #[arg(long)]
        suffix: Option<String>,
    },

    /// Link the current git branch to a story.
    ///
    /// Adds the branch to the story note's `branches:`, creating the story note if it doesn't exist.
    /// Trunk branches (main, master, develop, trunk and origin's default branch) can't be linked.
    #[command()]
    Link {
        /// Shortcut story id (`123` or `sc-123`).
        id: StoryId,
        /// Instead of linking, rename the current branch to the story's canonical name (`git branch -m`)
        /// and update the branch name in the story note's `branches:`.
        #[arg(
            long = "rename",
            action = ArgAction::SetTrue,
            value_parser = BoolValueParser::new().map(LinkMode::from_rename_flag),
        )]
        mode: LinkMode,
    },

    /// Append a session handoff entry to the story note's `## Sessions` section.
    ///
    /// The entry is headed with the local time, its UTC offset and the current git branch (when
    /// that branch belongs to the story). Creates the section, and the story note, if missing.
    /// Multi-line values are indented under their field.
    #[command()]
    Log {
        /// Shortcut story id (`123` or `sc-123`). Defaults to the story resolved from the current git branch.
        #[arg(long)]
        story: Option<StoryId>,
        /// What this session got done.
        #[arg(long)]
        done: EntryText,
        /// Where things stand now.
        #[arg(long)]
        state: EntryText,
        /// The next concrete steps.
        #[arg(long)]
        next: EntryText,
        /// Unresolved questions or decisions.
        #[arg(long)]
        questions: Option<EntryText>,
    },

    /// Claude Code hooks: story context at session start, stacked branch links and session handoffs.
    ///
    /// The event subcommands read the hook's JSON on stdin and print nothing when the hook's `cwd`
    /// doesn't resolve to a story or anything goes wrong.
    #[command(subcommand)]
    Hook(HookCommands),
}

#[derive(Subcommand)]
pub enum HookCommands {
    /// SessionStart: print the story context and arc workflow instructions, and record the session.
    SessionStart,
    /// PostToolUse (Bash): link branches created from a story branch to its story, or nudge to link them.
    PostBash,
    /// Stop: once per session, ask for a `log_session` handoff when there is unlogged work.
    Stop,
    /// SessionEnd: append an automatic `## Sessions` breadcrumb when work ended without a handoff.
    SessionEnd,
    /// Add the arc hooks to Claude Code's user settings and register the arc MCP server.
    Install(InstallArgs),
    /// Remove the arc hooks from Claude Code's user settings and unregister the arc MCP server.
    Uninstall(InstallArgs),
}

#[derive(Args)]
pub struct InstallArgs {
    /// Print the resulting settings and the `claude mcp` command without writing or running anything.
    #[arg(
        long = "dry-run",
        action = ArgAction::SetTrue,
        value_parser = BoolValueParser::new().map(install_mode),
    )]
    pub mode: Mode,
    /// Claude Code settings file to edit. Defaults to `~/.claude/settings.json`.
    #[arg(long)]
    pub settings: Option<PathBuf>,
    /// `claude` CLI used to (un)register the MCP server. Defaults to `claude` on PATH.
    #[arg(long)]
    pub claude: Option<PathBuf>,
}

fn install_mode(dry_run: bool) -> Mode {
    match dry_run {
        true => Mode::DryRun,
        false => Mode::Apply,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkMode {
    Link,
    RenameToCanonical,
}

impl LinkMode {
    fn from_rename_flag(rename: bool) -> Self {
        match rename {
            true => LinkMode::RenameToCanonical,
            false => LinkMode::Link,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum ContextFormat {
    /// Compact, human- and LLM-readable lines.
    Text,
    /// Pretty-printed JSON.
    Json,
}

#[derive(Subcommand)]
pub enum DaemonCommands {
    /// Run the daemon in the foreground. Invoked by the systemd user unit.
    Run,
    /// Install + enable the systemd user unit for the daemon.
    Install,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Commands, clap::Error> {
        Cli::try_parse_from(["arc"].iter().chain(args))
            .map(|cli| cli.command.expect("a subcommand"))
    }

    #[test]
    fn link_takes_prefixed_ids_and_a_rename_mode() {
        assert!(matches!(
            parse(&["link", "sc-12"]).unwrap(),
            Commands::Link { id, mode: LinkMode::Link } if id.get() == 12
        ));
        assert!(matches!(
            parse(&["link", "SC-12", "--rename"]).unwrap(),
            Commands::Link { id, mode: LinkMode::RenameToCanonical } if id.get() == 12
        ));
        assert!(parse(&["link", "0"]).is_err());
        assert!(parse(&["link", "sc-x"]).is_err());
    }

    #[test]
    fn log_takes_handoff_fields() {
        assert!(matches!(
            parse(&["log", "--story", "sc-7", "--done", "a", "--state", "b", "--next", "c"]).unwrap(),
            Commands::Log { story: Some(id), questions: None, .. } if id.get() == 7
        ));
        assert!(matches!(
            parse(&["log", "--done", "a", "--state", "b", "--next", "c", "--questions", "q"]).unwrap(),
            Commands::Log { story: None, questions: Some(_), .. }
        ));
        assert!(parse(&["log", "--done", " ", "--state", "b", "--next", "c"]).is_err());
        assert!(parse(&["log", "--done", "a", "--state", "b"]).is_err());
    }

    #[test]
    fn hook_subcommands_parse() {
        assert!(matches!(
            parse(&["hook", "post-bash"]).unwrap(),
            Commands::Hook(HookCommands::PostBash)
        ));
        assert!(matches!(
            parse(&["hook", "install", "--dry-run", "--settings", "/tmp/s.json"]).unwrap(),
            Commands::Hook(HookCommands::Install(InstallArgs {
                mode: Mode::DryRun,
                settings: Some(_),
                claude: None
            }))
        ));
        assert!(matches!(
            parse(&["hook", "uninstall"]).unwrap(),
            Commands::Hook(HookCommands::Uninstall(InstallArgs { mode: Mode::Apply, .. }))
        ));
        assert!(parse(&["hook", "nope"]).is_err());
    }

    #[test]
    fn branch_and_active_parse_story_ids() {
        assert!(matches!(
            parse(&["branch", "sc-7", "--suffix", "v2"]).unwrap(),
            Commands::Branch { id: Some(id), suffix: Some(_) } if id.get() == 7
        ));
        assert!(matches!(
            parse(&["branch"]).unwrap(),
            Commands::Branch { id: None, .. }
        ));
        assert!(parse(&["branch", "-3"]).is_err());
        assert!(matches!(
            parse(&["active", "42"]).unwrap(),
            Commands::Active { id } if id.get() == 42
        ));
    }
}
