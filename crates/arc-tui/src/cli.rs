use clap::{Parser, Subcommand};

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
}

#[derive(Subcommand)]
pub enum DaemonCommands {
    /// Run the daemon in the foreground. Invoked by the systemd user unit.
    Run,
    /// Install + enable the systemd user unit for the daemon.
    Install,
}
