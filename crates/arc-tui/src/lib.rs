use std::{fs, process::Command};

use anyhow::Context;
use ratatui::DefaultTerminal;

use crate::{
    api::{ApiClient, story::Story},
    app::{
        App,
        cmd::{NoteToOpen, open_mux_session},
    },
    cache::Cache,
    cli::{Commands, DaemonCommands},
    note::Note,
};
use arc_core::Config;
use arc_shortcut::fetch_story;

pub use arc_shortcut::{api, cache};
pub use arc_notes::{note, scanner, todos};

pub mod app;
pub mod cli;
pub mod custom_list;
pub mod dummy;
pub mod error;
pub mod keybindings;
pub mod text_utils;
pub mod tmux;
pub mod view;
pub mod zellij;
pub mod worktree;
#[macro_use]
pub mod keys;

macro_rules! no_active_story {
    () => {
        anyhow::bail!("You do not have a currently active story");
    };
}

pub async fn run(terminal: &mut DefaultTerminal) -> anyhow::Result<()> {
    let mut app = App::init().await?;
    app.main_loop(terminal).await?;
    app.model.config.write()?;

    Ok(())
}

pub async fn handle_command(
    command: Commands,
    cache: Cache,
    config: &Config,
) -> anyhow::Result<()> {
    match command {
        Commands::Note => {
            if let Some(story) = &cache.active_story {
                let iteration_app_url = cache.iteration_for(story).map(|it| it.app_url.clone());

                NoteToOpen::story(story, iteration_app_url).open(config)
            } else {
                no_active_story!();
            }
        }

        Commands::Tmux => {
            if let Some(story) = &cache.active_story {
                let session_name = Story::tmux_session_name(&story.name);
                open_mux_session(&session_name, &config.mux).await?;
                Ok(())
            } else {
                no_active_story!();
            }
        }

        Commands::ClearCache => {
            let cache_file = Cache::get_cache_file(config.cache_dir.clone());
            fs::remove_file(cache_file)?;
            Ok(())
        }

        Commands::Open => {
            if let Some(story) = &cache.active_story {
                open::that(story.app_url.clone())
                    .with_context(|| format!("Failed to open {}", story.app_url))
            } else {
                no_active_story!();
            }
        }

        Commands::Cat => {
            if let Some(story) = &cache.active_story {
                let iteration_app_url = cache.iteration_for(story).map(|it| it.app_url.clone());

                let note = Note::new(
                    &config.notes_dir,
                    story.id,
                    story.name.clone(),
                    story.app_url.clone(),
                    iteration_app_url,
                );

                let status = Command::new("cat").arg(note.path).status()?;

                if !status.success() {
                    anyhow::bail!("Failed to cat not for story {}", story.name);
                }

                Ok(())
            } else {
                no_active_story!();
            }
        }

        Commands::Active { id } => {
            let mut cache = cache;
            let story = match cache.find_story(id.into()) {
                Some(story) => story.clone(),
                None => {
                    let client =
                        ApiClient::from_token_and_cache(&config.api_token, &mut cache).await?;
                    fetch_story(&mut cache, &client, id.into())
                        .await
                        .with_context(|| format!("Failed to fetch story {id}"))?
                }
            };
            cache.active_story = Some(story);
            cache.write().await?;
            Ok(())
        }

        Commands::Daemon(daemon_cmd) => match daemon_cmd {
            DaemonCommands::Run => arc_daemon::run(config.clone()).await,
            DaemonCommands::Install => arc_daemon::install::install(),
        },

        Commands::Mcp => arc_mcp::run_stdio(config).await,

        Commands::Migrate => unreachable!("Migrate is handled in main.rs before config is loaded"),
    }
}
