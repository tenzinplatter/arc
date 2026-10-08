use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use chrono::{Datelike, Local, TimeZone};
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use arc_core::config::{Config, NotifierConfig};
use crate::ntfy::NtfyNotifier;
use crate::scheduler::{DBusNotifier, NoNotifier, Notifier, ScheduledTodo, Scheduler, SystemClock};
use crate::watcher::WatchEvent;
use arc_notes::NoteRelPath;
use arc_notes::todos::Todo;

pub mod install;
pub mod ntfy;
pub mod scheduler;
pub mod watcher;

const MAX_IDLE: std::time::Duration = std::time::Duration::from_secs(60);

pub async fn run(config: Config) -> Result<()> {
    init_tracing();
    info!("arc daemon starting; notes_dir={}", config.notes_dir.display());

    if !config.notes_dir.exists() {
        info!("notes_dir {} missing, creating", config.notes_dir.display());
        std::fs::create_dir_all(&config.notes_dir)?;
    }

    let mut index = arc_index::open(&config.cache_dir)?;
    bootstrap_scan(&config.notes_dir, &config.cache_dir, &mut index).await?;

    let (tx, mut rx) = mpsc::unbounded_channel::<WatchEvent>();
    let _watcher = watcher::spawn(config.notes_dir.clone(), tx)?;

    let mut sched = Scheduler::new(Arc::new(SystemClock), build_notifier(&config.notifier)?);
    rebuild_schedule(&config.cache_dir, &mut sched).await?;
    sched.tick();

    loop {
        let sleep_for = match sched.next_fire() {
            Some(at) => (at - Local::now())
                .to_std()
                .unwrap_or(std::time::Duration::ZERO)
                .min(MAX_IDLE),
            None => MAX_IDLE,
        };

        tokio::select! {
            event = rx.recv() => {
                let Some(event) = event else {
                    warn!("watcher channel closed; exiting");
                    return Ok(());
                };
                if let Err(e) = handle_event(&config, &mut sched, &mut index, event).await {
                    error!("event handling failed: {e:#}");
                }
            }
            _ = tokio::time::sleep(sleep_for) => {
                sched.tick();
            }
        }
    }
}

fn build_notifier(config: &NotifierConfig) -> Result<Arc<dyn Notifier>> {
    let notifier: Arc<dyn Notifier> = match config {
        NotifierConfig::Desktop => Arc::new(DBusNotifier),
        NotifierConfig::Ntfy { url, topic, token } => {
            info!("reminders go to ntfy topic {topic} at {url}");
            Arc::new(NtfyNotifier::new(
                tokio::runtime::Handle::current(),
                url,
                topic,
                token.as_deref(),
            )?)
        }
        NotifierConfig::Silent => {
            info!("reminders disabled by config");
            Arc::new(NoNotifier)
        }
    };
    Ok(notifier)
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_env("ARC_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
}

async fn bootstrap_scan(
    notes_dir: &Path,
    cache_dir: &Path,
    index: &mut arc_index::Connection,
) -> Result<()> {
    let files = list_markdown(notes_dir)?;
    info!("bootstrap: scanning {} note files", files.len());
    let mut todos = Vec::new();
    for abs in files {
        let Some(rel) = NoteRelPath::from_abs(notes_dir, &abs) else {
            warn!("bootstrap: skipping {}: not a UTF-8 path under the notes dir", abs.display());
            continue;
        };
        let content = match std::fs::read_to_string(&abs) {
            Ok(c) => c,
            Err(e) => {
                warn!("bootstrap read {} failed: {e}", abs.display());
                continue;
            }
        };
        todos.extend(arc_notes::scanner::parse_note(&content, &rel));
        if let Err(e) = arc_index::reindex_note(index, &rel, &content) {
            warn!("index {rel} failed: {e:#}");
        }
    }
    arc_notes::todos::save_todos(cache_dir, &todos).await?;
    mirror_todos(cache_dir, index).await;
    Ok(())
}

async fn mirror_todos(cache_dir: &Path, index: &mut arc_index::Connection) {
    let todos = arc_notes::todos::load_todos(cache_dir).await;
    if let Err(e) = arc_index::mirror_todos(index, &todos) {
        warn!("todo mirror failed: {e:#}");
    }
}

fn list_markdown(dir: &Path) -> Result<Vec<PathBuf>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let nested = std::fs::read_dir(dir)?
        .map(|entry| {
            let p = entry?.path();
            if p.is_dir() {
                list_markdown(&p)
            } else {
                Ok(watcher::is_markdown(&p).then_some(p).into_iter().collect())
            }
        })
        .collect::<Result<Vec<Vec<PathBuf>>>>()?;
    Ok(nested.concat())
}

fn read_current_content(event: &WatchEvent) -> std::io::Result<Option<String>> {
    match event {
        WatchEvent::Removed(_) => Ok(None),
        WatchEvent::Changed(abs) => match std::fs::read_to_string(abs) {
            Ok(content) => Ok(Some(content)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        },
    }
}

async fn handle_event(
    config: &Config,
    sched: &mut Scheduler,
    index: &mut arc_index::Connection,
    event: WatchEvent,
) -> Result<()> {
    let abs = event.path();
    let Some(rel) = NoteRelPath::from_abs(&config.notes_dir, abs) else {
        warn!("skipping {}: not a UTF-8 path under the notes dir", abs.display());
        return Ok(());
    };

    match read_current_content(&event) {
        Ok(None) => {
            arc_notes::todos::refresh_note(&config.cache_dir, &rel, None).await?;
            let _ = arc_index::remove_note(index, &rel);
        }
        Ok(Some(content)) => {
            arc_notes::todos::refresh_note(&config.cache_dir, &rel, Some(&content)).await?;
            if let Err(e) = arc_index::reindex_note(index, &rel, &content) {
                warn!("index {rel} failed: {e:#}");
            }
        }
        Err(e) => {
            warn!("read {} failed: {e}", abs.display());
            return Ok(());
        }
    }
    mirror_todos(&config.cache_dir, index).await;
    rebuild_schedule(&config.cache_dir, sched).await?;
    Ok(())
}

async fn rebuild_schedule(cache_dir: &Path, sched: &mut Scheduler) -> Result<()> {
    let todos = arc_notes::todos::load_todos(cache_dir).await;
    sched.clear();
    for scheduled in todos.into_iter().filter_map(scheduled_todo) {
        sched.upsert(scheduled);
    }
    Ok(())
}

fn scheduled_todo(t: Todo) -> Option<ScheduledTodo> {
    let date = t.date.filter(|_| !t.completed)?;
    let fire_at = Local
        .with_ymd_and_hms(date.year(), date.month(), date.day(), 9, 0, 0)
        .single()
        .unwrap_or_else(Local::now);
    let title = t
        .file
        .file_stem()
        .map_or_else(|| "arc".to_string(), |s| format!("arc — {s}"));
    Some(ScheduledTodo {
        id: t.id,
        title,
        body: t.text,
        fire_at,
    })
}
