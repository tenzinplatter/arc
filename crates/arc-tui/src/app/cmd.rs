use std::path::Path;
use std::{
    fs::{File, create_dir_all, read_to_string},
    process::Command,
};

use slugify::slugify;

use arc_notes::{Checkbox, NoteDir, NoteRelPath};

use crate::{
    api::{epic::EpicSlim, iteration::Iteration, story::Story},
    error::ErrorInfo,
    note::frontmatter::Frontmatter,
    tmux, zellij,
};
use arc_core::{Config, IterationId, Mux, StoryId, dbg_file};

#[derive(Debug, Clone)]
pub enum Cmd {
    None,
    OpenNote(NoteToOpen),
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
    CloseActionMenu,
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
    WriteTodos,
    SyncNoteCheckbox {
        file: NoteRelPath,
        text: String,
        to: Checkbox,
    },
    ShowError(ErrorInfo),
}

impl Cmd {
    pub fn open_iteration_note(iteration: Option<&Iteration>) -> Self {
        match iteration {
            Some(it) => Cmd::OpenNote(NoteToOpen::iteration(it)),
            None => Cmd::ShowError(ErrorInfo::new(
                "No iteration",
                "This story has no associated iteration",
            )),
        }
    }
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

fn obsidian_id(path: &NoteRelPath) -> String {
    path.file_stem().unwrap_or_default().to_string()
}

#[derive(Debug, Clone)]
pub struct NoteToOpen {
    pub path: NoteRelPath,
    pub template: Frontmatter,
}

impl NoteToOpen {
    pub fn story(story: &Story, iteration_app_url: Option<String>) -> Self {
        let slug = slugify!(&story.name);
        Self {
            path: NoteRelPath::in_dir(NoteDir::Stories, &slug),
            template: Frontmatter::story(
                slug,
                story.id,
                story.name.clone(),
                story.app_url.clone(),
                iteration_app_url,
            ),
        }
    }

    pub fn iteration(iteration: &Iteration) -> Self {
        let slug = slugify!(&iteration.name);
        Self {
            path: NoteRelPath::in_dir(NoteDir::Iterations, &slug),
            template: Frontmatter::iteration(
                slug,
                iteration.id,
                iteration.name.clone(),
                iteration.app_url.clone(),
            ),
        }
    }

    pub fn epic(epic: &EpicSlim) -> Self {
        let slug = slugify!(&epic.name);
        Self {
            path: NoteRelPath::in_dir(NoteDir::Epics, &slug),
            template: Frontmatter::epic(slug, epic.id, epic.name.clone(), epic.app_url.clone()),
        }
    }

    pub fn daily(date: chrono::NaiveDate) -> Self {
        Self::existing_or_daily_template(NoteRelPath::in_dir(NoteDir::Daily, &date.to_string()))
    }

    pub fn scratch(name: &str) -> Self {
        let path = NoteRelPath::in_dir(NoteDir::Scratch, &slugify!(name));
        let template = Frontmatter::scratch(obsidian_id(&path));
        Self { path, template }
    }

    pub fn existing_or_daily_template(path: NoteRelPath) -> Self {
        let template = Frontmatter::daily(obsidian_id(&path));
        Self { path, template }
    }

    pub fn open(self, config: &Config) -> anyhow::Result<()> {
        let path = self.path.abs(&config.notes_dir);
        if let Some(p) = path.parent() {
            create_dir_all(p)?;
        }
        let empty = !path.is_file() || read_to_string(&path)?.is_empty();
        if empty {
            std::fs::write(&path, self.template.to_block()?)?;
        }
        open_in_editor(config, &path)
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn resolved(note: &NoteToOpen) -> std::path::PathBuf {
        note.path.abs(Path::new("/notes"))
    }

    fn block(fm: &Frontmatter) -> String {
        fm.to_block().unwrap()
    }

    #[test]
    fn entity_notes_live_in_their_slugged_dirs() {
        let story = Story {
            id: 7.into(),
            name: "Fix The Thing".into(),
            app_url: "s-url".into(),
            ..Default::default()
        };
        let note = NoteToOpen::story(&story, Some("it-url".into()));
        assert_eq!(resolved(&note), Path::new("/notes/stories/fix-the-thing.md"));
        let expected = Frontmatter::story(
            "fix-the-thing".into(),
            7.into(),
            "Fix The Thing".into(),
            "s-url".into(),
            Some("it-url".into()),
        );
        assert_eq!(block(&note.template), block(&expected));
    }

    #[test]
    fn daily_and_scratch_templates_use_file_stem_as_id() {
        let date = chrono::NaiveDate::from_ymd_opt(2026, 1, 2).unwrap();
        let daily = NoteToOpen::daily(date);
        assert_eq!(resolved(&daily), Path::new("/notes/daily/2026-01-02.md"));
        assert_eq!(block(&daily.template), block(&Frontmatter::daily("2026-01-02".into())));

        let scratch = NoteToOpen::scratch("My Idea");
        assert_eq!(resolved(&scratch), Path::new("/notes/scratch/my-idea.md"));
        assert_eq!(block(&scratch.template), block(&Frontmatter::scratch("my-idea".into())));
    }

    #[test]
    fn existing_note_seeds_daily_template_from_its_stem() {
        let note =
            NoteToOpen::existing_or_daily_template(NoteRelPath::parse("stories/foo.md").unwrap());
        assert_eq!(resolved(&note), Path::new("/notes/stories/foo.md"));
        assert_eq!(block(&note.template), block(&Frontmatter::daily("foo".into())));
    }
}
