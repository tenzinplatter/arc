use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc as std_mpsc;
use std::time::{Duration, Instant};

use anyhow::Result;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc::UnboundedSender;

const DEBOUNCE_WINDOW: Duration = Duration::from_millis(500);

#[derive(Debug, Clone)]
pub enum WatchEvent {
    Changed(PathBuf),
    Removed(PathBuf),
}

impl WatchEvent {
    pub fn path(&self) -> &Path {
        match self {
            WatchEvent::Changed(p) | WatchEvent::Removed(p) => p,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum WatchKind {
    Changed,
    Removed,
}

impl WatchKind {
    fn of(kind: &EventKind) -> Self {
        match kind {
            EventKind::Remove(_) => WatchKind::Removed,
            _ => WatchKind::Changed,
        }
    }

    fn event(self, path: PathBuf) -> WatchEvent {
        match self {
            WatchKind::Changed => WatchEvent::Changed(path),
            WatchKind::Removed => WatchEvent::Removed(path),
        }
    }
}

struct Pending {
    last_seen: Instant,
    kind: WatchKind,
}

pub fn spawn(
    notes_dir: PathBuf,
    tx: UnboundedSender<WatchEvent>,
) -> Result<RecommendedWatcher> {
    let (notify_tx, notify_rx) = std_mpsc::channel();
    let mut watcher = notify::recommended_watcher(notify_tx)?;
    watcher.watch(&notes_dir, RecursiveMode::Recursive)?;

    std::thread::spawn(move || debounce_loop(notify_rx, tx));

    Ok(watcher)
}

fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("md"))
}

fn debounce_loop(
    rx: std_mpsc::Receiver<notify::Result<notify::Event>>,
    tx: UnboundedSender<WatchEvent>,
) {
    let mut pending: HashMap<PathBuf, Pending> = HashMap::new();
    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Ok(ev)) => ingest(&ev, &mut pending),
            Ok(Err(_)) | Err(std_mpsc::RecvTimeoutError::Timeout) => {}
            Err(std_mpsc::RecvTimeoutError::Disconnected) => return,
        }
        while let Ok(maybe_ev) = rx.try_recv() {
            if let Ok(ev) = maybe_ev {
                ingest(&ev, &mut pending);
            }
        }
        for ev in take_settled(&mut pending, Instant::now()) {
            if tx.send(ev).is_err() {
                return;
            }
        }
    }
}

fn take_settled(pending: &mut HashMap<PathBuf, Pending>, now: Instant) -> Vec<WatchEvent> {
    let mut settled = Vec::new();
    pending.retain(|path, p| {
        let is_settled = now.duration_since(p.last_seen) >= DEBOUNCE_WINDOW;
        if is_settled {
            settled.push(p.kind.event(path.clone()));
        }
        !is_settled
    });
    settled
}

fn ingest(ev: &notify::Event, pending: &mut HashMap<PathBuf, Pending>) {
    let kind = WatchKind::of(&ev.kind);
    for path in ev.paths.iter().filter(|p| is_markdown(p)) {
        pending.insert(
            path.clone(),
            Pending {
                last_seen: Instant::now(),
                kind,
            },
        );
    }
}
