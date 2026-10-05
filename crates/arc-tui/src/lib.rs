use std::{
    env, fs,
    io::{self, Write},
    process::Command,
};

use anyhow::Context;
use ratatui::DefaultTerminal;

use crate::{
    api::{ApiClient, story::Story},
    app::{
        App,
        cmd::{NoteToOpen, open_mux_session},
    },
    cache::Cache,
    cli::{Commands, ContextFormat, DaemonCommands, HookCommands, LinkMode, SettingsArgs},
    note::Note,
};
use arc_context::{Handoff, LinkOutcome, NoteRename, StoryNote};
use arc_core::{Config, dbg_file};
use arc_hooks::Hook;
use arc_hooks::install::{ArcExe, Claude, HookBinary, Installer};
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

async fn render_context(
    format: ContextFormat,
    config: &Config,
    cache: Cache,
) -> anyhow::Result<Option<String>> {
    let mut cache = cache.with_cache_dir(config.cache_dir.clone());
    let dir = env::current_dir()?;
    let Some(context) = arc_context::current_context(&dir, config, &mut cache).await? else {
        return Ok(None);
    };
    Ok(Some(match format {
        ContextFormat::Text => context.to_string(),
        ContextFormat::Json => format!("{}\n", serde_json::to_string_pretty(&context)?),
    }))
}

fn emit_context(rendered: anyhow::Result<Option<String>>) {
    match rendered {
        Ok(Some(context)) => {
            let _ = io::stdout().write_all(context.as_bytes());
        }
        Ok(None) => {}
        Err(e) => dbg_file!("arc context: {e:#}"),
    }
}

pub async fn print_context(format: ContextFormat) {
    std::panic::set_hook(Box::new(|panic| dbg_file!("arc context: {panic}")));
    let rendered = tokio::spawn(async move {
        let config = Config::read()?;
        let cache = Cache::read(config.cache_dir.clone()).await;
        render_context(format, &config, cache).await
    })
    .await
    .map_err(anyhow::Error::from)
    .and_then(|rendered| rendered);
    emit_context(rendered);
}

const CLAUDE_SETTINGS: &str = ".claude/settings.json";

fn installer(args: SettingsArgs) -> anyhow::Result<Installer> {
    let settings = match args.settings {
        Some(settings) => settings,
        None => env::home_dir()
            .context("can't find your home directory; pass --settings")?
            .join(CLAUDE_SETTINGS),
    };
    Ok(Installer {
        settings,
        claude: Claude::locate(args.claude),
    })
}

fn running_arc() -> anyhow::Result<ArcExe> {
    env::current_exe()
        .and_then(|exe| exe.canonicalize())
        .map(ArcExe::classify)
        .context("can't locate the running arc executable")
}

pub async fn hook(command: HookCommands) -> anyhow::Result<()> {
    let event = match command {
        HookCommands::SessionStart => Hook::SessionStart,
        HookCommands::PostBash => Hook::PostBash,
        HookCommands::Stop => Hook::Stop,
        HookCommands::SessionEnd => Hook::SessionEnd,
        HookCommands::Install(args) => {
            let arc = HookBinary::choose(running_arc()?, ArcExe::on_path(), args.dev_builds)?;
            let mode = args.target.mode;
            return installer(args.target)?.install(&arc, mode, &mut io::stdout());
        }
        HookCommands::Uninstall(args) => {
            let mode = args.mode;
            return installer(args)?.uninstall(mode, &mut io::stdout());
        }
    };
    arc_hooks::run(event).await
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
            let story = match cache.find_story(id) {
                Some(story) => story.clone(),
                None => {
                    let client =
                        ApiClient::from_token_and_cache(&config.api_token, &mut cache).await?;
                    fetch_story(&mut cache, &client, id)
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

        Commands::Context { format } => {
            emit_context(render_context(format, config, cache).await);
            Ok(())
        }

        Commands::Branch { id, suffix } => {
            let mut cache = cache.with_cache_dir(config.cache_dir.clone());
            let id = match id {
                Some(id) => id,
                None => {
                    arc_context::resolve_dir(&env::current_dir()?, config)?
                        .context("the current branch doesn't resolve to a story; pass a story id")?
                        .id
                }
            };
            let branch =
                arc_context::branch_for_story(config, &mut cache, id, suffix.as_deref()).await?;
            println!("{branch}");
            Ok(())
        }

        Commands::Link {
            id,
            mode: LinkMode::Link,
        } => {
            let mut cache = cache.with_cache_dir(config.cache_dir.clone());
            let dir = env::current_dir()?;
            let (branch, note, outcome) =
                arc_context::link_current_branch(&dir, config, &mut cache, id).await?;
            match outcome {
                LinkOutcome::Added => println!("linked {branch} to sc-{id} in {note}"),
                LinkOutcome::AlreadyLinked => {
                    println!("{branch} is already linked to sc-{id} in {note}")
                }
            }
            Ok(())
        }

        Commands::Link {
            id,
            mode: LinkMode::RenameToCanonical,
        } => {
            let mut cache = cache.with_cache_dir(config.cache_dir.clone());
            let dir = env::current_dir()?;
            let renamed = arc_context::rename_to_canonical(&dir, config, &mut cache, id).await?;
            println!("renamed {} to {}", renamed.from, renamed.to);
            match renamed.note {
                NoteRename::NotListed => {}
                NoteRename::Updated(note) => println!("updated branches in {note}"),
                NoteRename::Failed(note, e) => eprintln!(
                    "couldn't update branches in {note} ({e:#}); replace {} with {} there by hand",
                    renamed.from, renamed.to
                ),
            }
            Ok(())
        }

        Commands::Log {
            story,
            done,
            state,
            next,
            questions,
        } => {
            let mut cache = cache.with_cache_dir(config.cache_dir.clone());
            let handoff = Handoff {
                done,
                state,
                next,
                questions,
            };
            let logged = arc_context::log_session(&env::current_dir()?, config, &mut cache, story, handoff)
                .await?;
            if let StoryNote::Created { rel, .. } = &logged.note {
                println!("created {rel} for sc-{}", logged.story);
            }
            println!("logged to {}: {}", logged.note.rel(), logged.entry.header());
            Ok(())
        }

        Commands::Migrate => unreachable!("Migrate is handled in main.rs before config is loaded"),
        Commands::Hook(_) => unreachable!("Hook is handled in main.rs before config is loaded"),
    }
}
