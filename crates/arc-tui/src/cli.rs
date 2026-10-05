use arc_core::StoryId;
use clap::builder::{BoolValueParser, TypedValueParser};
use clap::{ArgAction, Parser, Subcommand, ValueEnum};

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
