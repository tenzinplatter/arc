use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use chrono::{Datelike, Local, TimeZone};
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use arc_core::config::Config;
use crate::scheduler::{DBusNotifier, ScheduledTodo, Scheduler, SystemClock};
use crate::watcher::WatchEvent;
use arc_notes::NoteRelPath;
use arc_notes::todos::TodoSource;

pub mod install;
pub mod scheduler;
pub mod store;
pub mod watcher;

pub async fn run(config: Config) -> Result<()> {
    init_tracing();
    info!("arc daemon starting; notes_dir={}", config.notes_dir.display());

    if !config.notes_dir.exists() {
        info!("notes_dir {} missing, creating", config.notes_dir.display());
        std::fs::create_dir_all(&config.notes_dir)?;
    }

    // Queryable index (writer connection; lives for the daemon's lifetime).
    let mut index = arc_index::open(&config.cache_dir)?;

    // Bootstrap: full scan.
    bootstrap_scan(&config.notes_dir, &config.cache_dir, &mut index).await?;

    // Spawn the watcher.
    let (tx, mut rx) = mpsc::unbounded_channel::<WatchEvent>();
    let _watcher = watcher::spawn(config.notes_dir.clone(), tx)?;

    // Scheduler.
    let clock = Arc::new(SystemClock);
    let notifier = Arc::new(DBusNotifier);
    let mut sched = Scheduler::new(clock, notifier);
    rebuild_schedule(&config.cache_dir, &mut sched).await?;
    // First tick handles overdue batching.
    sched.tick();

    // Main loop.
    loop {
        let sleep_for = match sched.next_fire() {
            Some(at) => {
                let now = Local::now();
                let dur = (at - now).to_std().unwrap_or(std::time::Duration::ZERO);
                dur.min(std::time::Duration::from_secs(60))
            }
            None => std::time::Duration::from_secs(60),
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
    for abs in files {
        let Some(rel) = NoteRelPath::from_abs(notes_dir, &abs) else {
            warn!("bootstrap: skipping {} outside notes dir", abs.display());
            continue;
        };
        let content = match std::fs::read_to_string(&abs) {
            Ok(c) => c,
            Err(e) => {
                warn!("bootstrap read {} failed: {e}", abs.display());
                continue;
            }
        };
        let parsed = arc_notes::scanner::parse_note(&content, &rel);
        if let Err(e) = store::merge_file(cache_dir, &rel, parsed).await {
            warn!("merge {rel} failed: {e:#}");
        }
        if let Err(e) = arc_index::reindex_note(index, &rel, &content) {
            warn!("index {rel} failed: {e:#}");
        }
    }
    mirror_todos(cache_dir, index).await;
    Ok(())
}

/// Refresh the read-only todo mirror from the authoritative `todos.json`.
async fn mirror_todos(cache_dir: &Path, index: &mut arc_index::Connection) {
    let todos = arc_notes::todos::load_todos(cache_dir).await;
    if let Err(e) = arc_index::mirror_todos(index, &todos) {
        warn!("todo mirror failed: {e:#}");
    }
}

fn list_markdown(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    if !dir.exists() {
        return Ok(out);
    }
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d)? {
            let entry = entry?;
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else if p
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("md"))
            {
                out.push(p);
            }
        }
    }
    Ok(out)
}

async fn handle_event(
    config: &Config,
    sched: &mut Scheduler,
    index: &mut arc_index::Connection,
    event: WatchEvent,
) -> Result<()> {
    let (abs, removed) = match event {
        WatchEvent::Changed(p) => (p, false),
        WatchEvent::Removed(p) => (p, true),
    };
    let Some(rel) = NoteRelPath::from_abs(&config.notes_dir, &abs) else {
        warn!("skipping {} outside notes dir", abs.display());
        return Ok(());
    };

    let treat_as_removed = removed
        || matches!(
            std::fs::symlink_metadata(&abs),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound
        );

    if treat_as_removed {
        let _ = store::drop_file(&config.cache_dir, &rel).await?;
        let _ = arc_index::remove_note(index, &rel);
    } else {
        let content = match std::fs::read_to_string(&abs) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Raced with delete/rename — treat as removal.
                let _ = store::drop_file(&config.cache_dir, &rel).await?;
                let _ = arc_index::remove_note(index, &rel);
                mirror_todos(&config.cache_dir, index).await;
                rebuild_schedule(&config.cache_dir, sched).await?;
                return Ok(());
            }
            Err(e) => {
                warn!("read {} failed: {e}", abs.display());
                return Ok(());
            }
        };
        let parsed = arc_notes::scanner::parse_note(&content, &rel);
        let _ = store::merge_file(&config.cache_dir, &rel, parsed).await?;
        if let Err(e) = arc_index::reindex_note(index, &rel, &content) {
            warn!("index {rel} failed: {e:#}");
        }
    }
    mirror_todos(&config.cache_dir, index).await;
    rebuild_schedule(&config.cache_dir, sched).await?;
    Ok(())
}

async fn rebuild_schedule(cache_dir: &Path, sched: &mut Scheduler) -> Result<()> {
    let todos = arc_notes::todos::load_todos(cache_dir).await;
    // Cancel everything we know about, then re-insert.
    sched.clear();
    for t in todos {
        if t.completed {
            continue;
        }
        let Some(date) = t.date else { continue };
        let fire_at = Local
            .with_ymd_and_hms(date.year(), date.month(), date.day(), 9, 0, 0)
            .single()
            .unwrap_or_else(Local::now);
        let title = match &t.source {
            TodoSource::NoteParsed { file, .. } => file
                .file_stem()
                .map(|s| format!("arc — {}", s))
                .unwrap_or_else(|| "arc".to_string()),
            TodoSource::Manual => "arc".to_string(),
        };
        sched.upsert(ScheduledTodo {
            id: t.id,
            title,
            body: t.text.clone(),
            fire_at,
        });
    }
    Ok(())
}
