use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use chrono::{Datelike, Local, TimeZone};
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use arc_core::config::{Config, NotifierConfig};
use crate::fired::FiredLog;
use crate::lock::DaemonLock;
use crate::ntfy::NtfyNotifier;
use crate::scheduler::{
    DBusNotifier, NoNotifier, Notifier, Outcome, ScheduledTodo, Scheduler, SystemClock,
};
use crate::watcher::WatchEvent;
use arc_notes::NoteRelPath;
use arc_notes::todos::Todo;

pub mod fired;
pub mod install;
pub mod lock;
pub mod ntfy;
pub mod scheduler;
pub mod watcher;

const SESSION_BUS_VAR: &str = "DBUS_SESSION_BUS_ADDRESS";
const MAX_IDLE: std::time::Duration = std::time::Duration::from_secs(60);

pub async fn run(config: Config) -> Result<()> {
    init_tracing();
    info!("arc daemon starting; notes_dir={}", config.notes_dir.display());
    let _lock = DaemonLock::acquire(&config.cache_dir)?;

    if !config.notes_dir.exists() {
        info!("notes_dir {} missing, creating", config.notes_dir.display());
        std::fs::create_dir_all(&config.notes_dir)?;
    }

    let mut index = arc_index::open(&config.cache_dir)?;
    let (tx, mut rx) = mpsc::unbounded_channel::<WatchEvent>();
    let _watcher = watcher::spawn(config.notes_dir.clone(), tx)?;
    bootstrap_scan(&config.notes_dir, &config.cache_dir, &mut index).await?;

    let (fired_log, previously_fired) =
        FiredLog::open(&config.cache_dir, Local::now().date_naive())?;
    let (outcome_tx, mut outcomes) = mpsc::unbounded_channel::<Outcome>();
    let mut sched = Scheduler::new(
        Arc::new(SystemClock),
        build_notifier(&config.notifier)?,
        outcome_tx,
        previously_fired,
    );
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
            Some(outcome) = outcomes.recv() => {
                if let Err(e) = fired_log.record(&sched.settle(outcome)) {
                    warn!("recording fired reminders failed: {e}");
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
        NotifierConfig::Desktop => {
            if std::env::var_os(SESSION_BUS_VAR).is_none() {
                warn!(
                    "{SESSION_BUS_VAR} is unset, so desktop reminders can't be shown until a session bus appears; \
                     set [notifier] kind = \"ntfy\" or \"none\" if this machine has no desktop session"
                );
            }
            Arc::new(DBusNotifier)
        }
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
    let notes = list_notes(notes_dir, notes_dir)?;
    info!("bootstrap: scanning {} note files", notes.len());
    let mut todos = Vec::new();
    for rel in notes {
        let abs = rel.abs(notes_dir);
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

fn list_notes(notes_dir: &Path, dir: &Path) -> Result<Vec<NoteRelPath>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let nested = std::fs::read_dir(dir)?
        .map(|entry| {
            let p = entry?.path();
            if p.is_dir() {
                list_notes(notes_dir, &p)
            } else {
                Ok(NoteRelPath::from_abs(notes_dir, &p).into_iter().collect())
            }
        })
        .collect::<Result<Vec<Vec<NoteRelPath>>>>()?;
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
    if refresh_note_from_event(&config.notes_dir, &config.cache_dir, index, &event).await? {
        mirror_todos(&config.cache_dir, index).await;
        rebuild_schedule(&config.cache_dir, sched).await?;
    }
    Ok(())
}

async fn refresh_note_from_event(
    notes_dir: &Path,
    cache_dir: &Path,
    index: &mut arc_index::Connection,
    event: &WatchEvent,
) -> Result<bool> {
    let abs = event.path();
    let Some(rel) = NoteRelPath::from_abs(notes_dir, abs) else {
        warn!("skipping {}: not a note under the notes dir", abs.display());
        return Ok(false);
    };

    match read_current_content(event) {
        Ok(None) => {
            arc_notes::todos::refresh_note(cache_dir, &rel, None).await?;
            let _ = arc_index::remove_note(index, &rel);
        }
        Ok(Some(content)) => {
            arc_notes::todos::refresh_note(cache_dir, &rel, Some(&content)).await?;
            if let Err(e) = arc_index::reindex_note(index, &rel, &content) {
                warn!("index {rel} failed: {e:#}");
            }
        }
        Err(e) => {
            warn!("read {} failed: {e}", abs.display());
            return Ok(false);
        }
    }
    Ok(true)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_lists_notes_but_not_sync_artefacts() {
        let notes = tempfile::tempdir().unwrap();
        for rel in [
            "todos.md",
            "stories/a.md",
            "todos.sync-conflict-20261009-000000-ABCDEFG.md",
            ".stversions/todos~20261009-000000.md",
            ".git/x.md",
            "stories/a.txt",
        ] {
            let abs = notes.path().join(rel);
            std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
            std::fs::write(abs, "- [ ] x\n").unwrap();
        }
        let mut listed: Vec<String> =
            list_notes(notes.path(), notes.path()).unwrap().iter().map(ToString::to_string).collect();
        listed.sort();
        assert_eq!(listed, ["stories/a.md", "todos.md"]);
    }

    #[tokio::test]
    async fn a_todo_added_while_the_bootstrap_scan_runs_survives_its_cache_replace() {
        let (notes, cache) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let mut index = arc_index::open(cache.path()).unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel::<WatchEvent>();
        let _watcher = watcher::spawn(notes.path().to_path_buf(), tx).unwrap();

        let added = arc_notes::todos::add_todo(notes.path(), cache.path(), "mid-scan".into(), None).await.unwrap();
        arc_notes::todos::save_todos(cache.path(), &[]).await.unwrap();

        let event = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv()).await.unwrap().unwrap();
        assert!(refresh_note_from_event(notes.path(), cache.path(), &mut index, &event).await.unwrap());
        assert_eq!(arc_notes::todos::load_todos(cache.path()).await, [added.todo]);
    }
}
