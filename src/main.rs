use arc_core::Config;
use arc_tui::cache::Cache;
use arc_tui::cli::{Cli, Commands};
use arc_tui::worktree::check_worktree_dependencies;
use clap::Parser;

mod migrate;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        None => {
            check_worktree_dependencies()?;
            let mut terminal = ratatui::init();
            let result = arc_tui::run(&mut terminal).await;
            ratatui::restore();
            result
        }
        Some(Commands::Migrate) => migrate::migrate(),
        Some(Commands::Hook(command)) => arc_tui::hook(command).await,
        Some(Commands::Context { format }) => {
            arc_tui::print_context(format).await;
            Ok(())
        }
        Some(cmd) => {
            let config = Config::read()?;
            let cache = Cache::read(config.cache_dir.clone()).await;
            arc_tui::handle_command(cmd, cache, &config).await
        }
    }
}
