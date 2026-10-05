use clap::{Parser, Subcommand, ValueEnum};

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
    Active { id: i32 },

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
        /// Shortcut story id. Defaults to the story resolved from the current git branch.
        id: Option<i32>,
        /// Slugified and appended to the story slug as `-{suffix}`.
        #[arg(long)]
        suffix: Option<String>,
    },

    /// Link the current git branch to a story.
    ///
    /// Adds the branch to the story note's `branches:`, creating the story note if it doesn't exist.
    #[command()]
    Link {
        /// Shortcut story id.
        id: i32,
        /// Instead of linking, rename the current branch to the story's canonical name (`git branch -m`).
        #[arg(long)]
        rename: bool,
    },
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
