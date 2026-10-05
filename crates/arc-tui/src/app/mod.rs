use std::fs::read_to_string;
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::ExecutableCommand;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::widgets::{Block, Clear, Paragraph, StatefulWidget, WidgetRef, Widget};
use ratatui::{DefaultTerminal, Frame};
use tempfile::NamedTempFile;
use tokio::sync::mpsc;

use crate::app::pane::action_menu::ActionMenu;
use crate::error::{ERROR_NOTIFICATION_MAX_HEIGHT, ErrorInfo};
use crate::view::add_todo_modal::AddTodoModal;
use crate::view::create_note_modal::CreateNoteModal;
use crate::view::description_modal::{DescriptionModal, centered_rect};
use crate::view::keybinds_panel::KeybindsPanel;
use crate::view::todos_list::TodosListView;
use crate::view::{EpicListView, IterationListView};
use crate::view::{navbar::NavBar, notes_list::NotesListView, story_list::StoryListView};
use crate::worktree::{create_worktree, get_repo_list, select_repo_with_fzf};
use crate::{
    api::{ApiClient, story::Story},
    app::model::{Overlay, ViewType},
};
use arc_core::Config;

pub mod cmd;
pub mod init;
pub mod model;
pub mod msg;
pub mod pane;
pub mod update;

const NAVBAR_HEIGHT: u16 = 3;
const KEY_READER_POLL: Duration = Duration::from_millis(100);
const KEY_READER_PAUSED_SLEEP: Duration = Duration::from_millis(20);
const SUSPEND_GRACE: Duration = Duration::from_millis(20);
const INPUT_DRAIN_WINDOW: Duration = Duration::from_millis(50);

pub struct App {
    pub model: model::Model,
    pub exit: bool,
    pub receiver: mpsc::UnboundedReceiver<msg::Msg>,
    pub sender: mpsc::UnboundedSender<msg::Msg>,
    pub api_client: ApiClient,
    pub config: Config,
    pub key_reader_paused: Arc<AtomicBool>,
}

impl App {
    pub async fn main_loop(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        while !self.exit {
            terminal.draw(|frame| self.draw(frame))?;

            if let Some(msg) = self.poll_for_message().await? {
                let commands = self.update(msg);

                for cmd in commands {
                    self.execute_cmd(cmd, terminal).await?;
                }
            }
        }

        Ok(())
    }

    async fn execute_cmd(
        &mut self,
        cmd: cmd::Cmd,
        terminal: &mut DefaultTerminal,
    ) -> Result<()> {
        use crate::app::msg::Msg;
        match cmd {
            cmd::Cmd::None => {}

            cmd::Cmd::Batch(commands) => {
                for cmd in commands {
                    Box::pin(self.execute_cmd(cmd, terminal)).await?;
                }
            }

            cmd::Cmd::WriteCache => {
                self.model.cache.write().await?;
                self.sender.send(Msg::CacheWritten).ok();
            }

            cmd::Cmd::FetchStories { iteration_ids } => {
                let sender = self.sender.clone();
                let api_client = self.api_client.clone();
                let handle = tokio::spawn(async move {
                    match api_client.get_owned_iteration_stories(iteration_ids).await {
                        Ok(stories) => {
                            sender.send(Msg::StoriesLoaded(stories)).ok();
                        }
                        Err(e) => {
                            let info = ErrorInfo::new(
                                "Failed to get stories for current iteration".to_string(),
                                e.to_string(),
                            );
                            sender.send(Msg::Error(info)).ok();
                        }
                    }
                });
                self.model.data.async_handles.push(handle);
            }

            cmd::Cmd::FetchEpics => {
                let sender = self.sender.clone();
                let api_client = self.api_client.clone();
                let handle = tokio::spawn(async move {
                    match api_client.get_all_epics_slim().await {
                        Ok(epics) => {
                            sender.send(Msg::EpicsLoaded(epics)).ok();
                        }
                        Err(e) => {
                            let info =
                                ErrorInfo::new("Failed to fetch epics".to_string(), e.to_string());
                            sender.send(Msg::Error(info)).ok();
                        }
                    }
                });
                self.model.data.async_handles.push(handle);
            }

            cmd::Cmd::SelectStory(story) => {
                if let Some(active_story) = &self.model.data.active_story
                    && let Some(story) = &story
                    && active_story.id == story.id
                {
                    self.model.cache.active_story = None;
                    self.model.data.active_story = None;
                } else {
                    self.model.data.active_story = story.clone();
                    self.model.cache.active_story = story;
                }
            }

            cmd::Cmd::CloseActionMenu => {
                if let Overlay::ActionMenu { .. } = self.model.ui.overlay {
                    self.model.ui.overlay = Overlay::None;
                }
            }

            cmd::Cmd::WriteTodos => {
                let todos_snapshot = self.model.data.todos.clone();
                crate::todos::modify_with_lock(&self.model.config.cache_dir, move |list| {
                    *list = todos_snapshot;
                })
                .await?;
            }

            cmd::Cmd::SyncNoteCheckbox {
                file,
                text,
                to,
            } => {
                let notes_dir = self.model.config.notes_dir.clone();
                let sender = self.sender.clone();
                tokio::spawn(async move {
                    if let Err(e) =
                        crate::todos::toggle_note_checkbox(&notes_dir, &file, &text, to).await
                    {
                        let info = ErrorInfo::new(
                            "Failed to sync todo state to note".to_string(),
                            e.to_string(),
                        );
                        let _ = sender.send(Msg::Error(info));
                    }
                });
            }

            cmd::Cmd::OpenInBrowser { app_url } => {
                open::that(&app_url)
                    .with_context(|| format!("Failed to open {} in browser", app_url))?;
            }

            cmd::Cmd::OpenNote(note) => {
                with_suspended_tui(&self.key_reader_paused, terminal, || {
                    note.open(&self.model.config)
                })?;
                self.sender.send(msg::Msg::NoteOpened).ok();
            }

            cmd::Cmd::EditStoryContent {
                story_id,
                description,
            } => {
                let config_editor = self.model.config.editor.clone();
                let edited = with_suspended_tui(&self.key_reader_paused, terminal,|| {
                    let mut tempfile = NamedTempFile::new()?;
                    tempfile.write_all(description.as_bytes())?;
                    let tmp_path = tempfile.path().to_path_buf();

                    std::process::Command::new(&config_editor)
                        .arg(&tmp_path)
                        .status()?;

                    let contents = read_to_string(&tmp_path)?;
                    Ok(contents)
                })?;

                if edited != description {
                    self.api_client
                        .update_story_description(story_id, edited)
                        .await?;
                }
            }

            cmd::Cmd::CreateGitWorktree { branch_name } => {
                let repos = get_repo_list(&self.model.config).await?;
                let chosen = match with_suspended_tui(&self.key_reader_paused, terminal,|| select_repo_with_fzf(&repos)) {
                    Ok(repo) => repo,
                    Err(e) => {
                        self.model
                            .ui
                            .errors
                            .push(ErrorInfo::new("Failed to get repo for worktree", e));
                        return Ok(());
                    }
                };

                let path = self.model.config.repositories_directory.join(chosen);
                create_worktree(&path, &branch_name).await?;
            }

            cmd::Cmd::OpenTmuxSession { story_name } => {
                let session_name = Story::tmux_session_name(&story_name);
                let mux = self.model.config.mux.clone();
                with_suspended_tui(&self.key_reader_paused, terminal,|| {
                    cmd::open_mux_session_sync(&session_name, &mux)
                })?;
            }
        }

        Ok(())
    }

    async fn poll_for_message(&mut self) -> Result<Option<msg::Msg>> {
        Ok(self.receiver.recv().await)
    }

    fn draw(&mut self, frame: &mut Frame) {
        self.model.ui.throbber_state.calc_next();
        let tick = self.model.ui.throbber_state.index().unsigned_abs() as usize;

        let [navbar_area, main_area] =
            Layout::vertical([Constraint::Length(NAVBAR_HEIGHT), Constraint::Min(0)])
                .areas(frame.area());

        let has_stories = !self.model.data.stories.is_empty();
        let navbar = NavBar::new(
            self.model.ui.active_view,
            self.model.ui.loading,
            has_stories,
            tick,
        );

        navbar.render_ref(navbar_area, frame.buffer_mut());

        match self.model.ui.active_view {
            ViewType::Stories => {
                let story_list_view = StoryListView::new(
                    &self.model.data.stories,
                    self.model.data.current_iterations.as_deref().unwrap_or(&[]),
                    &self.model.ui.story_list,
                    self.model.data.active_story.as_ref(),
                    self.model.ui.loading,
                    tick,
                );

                story_list_view.render_ref(main_area, frame.buffer_mut());
            }

            ViewType::Notes => {
                let notes_view = NotesListView::new(&self.model.ui.notes_list);
                notes_view.render_ref(main_area, frame.buffer_mut());
            }

            ViewType::Iterations => {
                let active = self.model.data.current_iterations.as_deref().unwrap_or(&[]);
                let iteration_list = IterationListView::new(
                    active,
                    &self.model.data.iterations,
                    &self.model.ui.iteration_list,
                );
                iteration_list.render_ref(main_area, frame.buffer_mut());
            }

            ViewType::Epics => {
                let epic_list =
                    EpicListView::new(&self.model.data.epics, &self.model.ui.epic_list);
                epic_list.render_ref(main_area, frame.buffer_mut());
            }

            ViewType::Todos => {
                let todos_view = TodosListView::new(
                    &self.model.data.todos,
                    &self.model.ui.todos_list,
                );
                todos_view.render_ref(main_area, frame.buffer_mut());
            }

            ViewType::Search => {
                let placeholder = Paragraph::new("Coming soon...").block(Block::bordered());
                placeholder.render(main_area, frame.buffer_mut());
            }
        }

        self.draw_overlay(frame);
        self.draw_error(frame);
    }

    fn draw_overlay(&mut self, frame: &mut Frame) {
        let area = frame.area();
        match &mut self.model.ui.overlay {
            Overlay::None => {}
            Overlay::Keybinds => KeybindsPanel.render(area, frame.buffer_mut()),
            Overlay::Description { story, scroll } => {
                let modal_area = centered_rect(80, 80, area);
                Clear.render(modal_area, frame.buffer_mut());
                DescriptionModal::new(story).render(modal_area, frame.buffer_mut(), scroll);
            }
            Overlay::ActionMenu { list, .. } => {
                let (width, height) = ActionMenu::window_dimensions();
                let width = (width as u16).min(area.width);
                let height = (height as u16).min(area.height);
                let x = (area.width - width) / 2;
                let y = (area.height - height) / 2;
                ActionMenu.render(Rect::new(x, y, width, height), frame.buffer_mut(), list);
            }
            Overlay::CreateNote { input } => {
                Clear.render(centered_rect(50, 30, area), frame.buffer_mut());
                CreateNoteModal::new(input).render_ref(area, frame.buffer_mut());
            }
            Overlay::AddTodo { input } => {
                Clear.render(centered_rect(50, 30, area), frame.buffer_mut());
                AddTodoModal::new(input).render_ref(area, frame.buffer_mut());
            }
        }
    }

    fn draw_error(&self, frame: &mut Frame) {
        let mut used_height = 0;
        for error in self.model.ui.errors.iter().filter(|e| !e.is_expired()) {
            let width = error.get_required_width();
            let height = error.get_required_height(width);
            let area = frame.area();
            let area = Rect::new(
                area.width - width,
                used_height,
                width,
                height.min(ERROR_NOTIFICATION_MAX_HEIGHT),
            );

            used_height += height;

            Clear.render(area, frame.buffer_mut());

            error.render_ref(area, frame.buffer_mut());
        }
    }
}

fn with_suspended_tui<F, T>(
    key_reader_paused: &AtomicBool,
    terminal: &mut DefaultTerminal,
    f: F,
) -> anyhow::Result<T>
where
    F: FnOnce() -> anyhow::Result<T>,
{
    key_reader_paused.store(true, Ordering::SeqCst);
    // An in-flight poll holds crossterm's reader lock; let it finish before toggling raw mode.
    std::thread::sleep(KEY_READER_POLL + SUSPEND_GRACE);
    std::io::stdout().execute(LeaveAlternateScreen)?;
    disable_raw_mode()?;
    let result = f();
    std::io::stdout().execute(EnterAlternateScreen)?;
    enable_raw_mode()?;
    terminal.clear()?;
    // Discard keys typed in the external program; bounded since a tty at EOF polls ready forever.
    let deadline = std::time::Instant::now() + INPUT_DRAIN_WINDOW;
    while std::time::Instant::now() < deadline
        && crossterm::event::poll(Duration::ZERO).unwrap_or(false)
        && crossterm::event::read().is_ok()
    {}
    key_reader_paused.store(false, Ordering::SeqCst);
    result
}

fn spawn_key_reader(sender: mpsc::UnboundedSender<msg::Msg>, paused: Arc<AtomicBool>) {
    use crossterm::event::{self, Event, KeyEventKind};

    std::thread::spawn(move || {
        loop {
            if paused.load(Ordering::SeqCst) {
                std::thread::sleep(KEY_READER_PAUSED_SLEEP);
                continue;
            }

            match event::poll(KEY_READER_POLL) {
                Ok(true) => match event::read() {
                    Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => {
                        if sender.send(msg::Msg::KeyPressed(key)).is_err() {
                            break;
                        }
                    }
                    Ok(_) => {}
                    Err(_) => break,
                },
                Ok(false) => {}
                Err(_) => break,
            }
        }
    });
}
