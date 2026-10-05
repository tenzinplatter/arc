use std::path::{Path, PathBuf};
use std::{
    fs::{File, create_dir_all, read_to_string},
    process::Command,
};

use slugify::slugify;

use arc_notes::NoteRelPath;

use crate::{
    api::story::Story,
    note::{Note, frontmatter::Frontmatter},
    tmux, zellij,
};
use arc_core::{Config, EpicId, IterationId, Mux, StoryId, dbg_file};

#[derive(Debug, Clone)]
pub enum Cmd {
    None,
    OpenNote {
        story_id: StoryId,
        story_name: String,
        story_app_url: String,
        iteration_app_url: Option<String>,
    },
    WriteCache,
    FetchStories {
        iteration_ids: Vec<IterationId>,
    },
    EditStoryContent {
        story_id: StoryId,
        description: String,
    },
    FetchEpics,
    SelectStory(Option<Story>),
    ActionMenuVisibility(bool),
    CreateGitWorktree {
        branch_name: String,
    },
    OpenTmuxSession {
        story_name: String,
    },
    Batch(Vec<Cmd>),
    OpenInBrowser {
        app_url: String,
    },
    OpenIterationNote {
        iteration_id: IterationId,
        iteration_name: String,
        iteration_app_url: String,
    },
    OpenEpicNote {
        epic_id: EpicId,
        epic_name: String,
        epic_app_url: String,
    },
    OpenDailyNote {
        path: PathBuf,
    },
    OpenScratchNote {
        path: PathBuf,
        name: String,
    },
    WriteTodos,
    SyncNoteCheckbox {
        file: NoteRelPath,
        text: String,
        complete: bool,
    },
}

pub fn open_in_editor(config: &Config, path: &Path) -> anyhow::Result<()> {
    if path.is_dir() {
        anyhow::bail!("Note path: {} is not a file", path.display());
    }

    if let Some(p) = path.parent() {
        create_dir_all(p)?;
    }

    if !path.is_file() {
        File::create(path)?;
    }

    dbg_file!("Opening in editor: {}", path.display());

    let res = Command::new(&config.editor).arg(path).status()?;

    if !res.success() {
        anyhow::bail!("Failed to open {} in editor", path.display());
    }

    Ok(())
}

/// Write `fm`'s frontmatter block iff the note is new or empty, then open it in
/// the editor. Single write path for every note type.
fn open_with_frontmatter(config: &Config, path: &Path, fm: Frontmatter) -> anyhow::Result<()> {
    if let Some(p) = path.parent() {
        create_dir_all(p)?;
    }
    let empty = !path.is_file() || read_to_string(path)?.is_empty();
    if empty {
        std::fs::write(path, fm.to_block()?)?;
    }
    open_in_editor(config, path)
}

pub fn open_note_in_editor(
    story_id: StoryId,
    story_name: String,
    story_app_url: String,
    iteration_app_url: Option<String>,
    config: &Config,
) -> anyhow::Result<()> {
    let note = Note::new(
        &config.notes_dir,
        story_id,
        story_name,
        story_app_url,
        iteration_app_url,
    );
    open_with_frontmatter(config, &note.path, note.frontmatter)
}

pub fn open_iteration_note_in_editor(
    iteration_id: IterationId,
    iteration_name: String,
    iteration_app_url: String,
    config: &Config,
) -> anyhow::Result<()> {
    let slug = slugify!(&iteration_name);
    let mut path = config.notes_dir.clone();
    path.push("iterations");
    path.push(format!("{slug}.md"));
    let fm = Frontmatter::iteration(slug, iteration_id, iteration_name, iteration_app_url);
    open_with_frontmatter(config, &path, fm)
}

pub fn open_epic_note_in_editor(
    epic_id: EpicId,
    epic_name: String,
    epic_app_url: String,
    config: &Config,
) -> anyhow::Result<()> {
    let slug = slugify!(&epic_name);
    let mut path = config.notes_dir.clone();
    path.push("epics");
    path.push(format!("{slug}.md"));
    let fm = Frontmatter::epic(slug, epic_id, epic_name, epic_app_url);
    open_with_frontmatter(config, &path, fm)
}

pub fn open_daily_note_with_frontmatter(config: &Config, path: &Path) -> anyhow::Result<()> {
    dbg_file!("Opening daily note in editor: {}", path.display());
    let slug = file_stem(path);
    open_with_frontmatter(config, path, Frontmatter::daily(slug))
}

pub fn open_scratch_note_in_editor(_name: &str, path: &Path, config: &Config) -> anyhow::Result<()> {
    let slug = file_stem(path);
    open_with_frontmatter(config, path, Frontmatter::scratch(slug))
}

/// Obsidian's `id` is the filename stem.
fn file_stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string()
}

pub async fn open_mux_session(name: &str, mux: &Mux) -> anyhow::Result<()> {
    match mux {
        Mux::Tmux => {
            if !tmux::session_exists(name).await? {
                tmux::session_create(name).await?;
            }
            tmux::session_attach(name).await?;
        }
        Mux::Zellij => {
            if !zellij::session_exists(name).await? {
                zellij::session_create(name).await?;
            }
            zellij::session_attach(name).await?;
        }
    }
    Ok(())
}

pub fn open_mux_session_sync(name: &str, mux: &Mux) -> anyhow::Result<()> {
    use std::process::Command;

    match mux {
        Mux::Tmux => {
            let out = Command::new("tmux")
                .args(["list-sessions", "-F", "#{session_name}"])
                .output()?;
            let exists = String::from_utf8_lossy(&out.stdout)
                .lines()
                .any(|l| l.trim() == name);

            if !exists {
                Command::new("tmux")
                    .args(["new-session", "-d", "-s", name])
                    .status()?;
            }

            let cmd = if tmux::attatched_to_session() {
                "switch-client"
            } else {
                "attach-session"
            };
            Command::new("tmux").args([cmd, "-t", name]).status()?;
        }
        Mux::Zellij => {
            let out = Command::new("zellij").arg("list-sessions").output();
            let exists = match out {
                Ok(out) if out.status.success() => {
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    stdout
                        .lines()
                        .any(|l| l.split_whitespace().next() == Some(name))
                }
                _ => false,
            };

            if !exists {
                Command::new("zellij")
                    .args(["attach", "--create-background", name])
                    .status()?;
            }

            Command::new("zellij").args(["attach", name]).status()?;
        }
    }
    Ok(())
}
