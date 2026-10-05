use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Style,
    text::Line,
    widgets::{Block, BorderType, Clear, Padding, StatefulWidget, Widget},
};
use tui_widget_list::{ListBuilder, ListState, ListView};

use crate::{
    api::story::{get_story_associated_iteration, Story},
    app::{
        cmd::Cmd,
        model::{DataState, UiState},
        msg::ActionMenuMsg,
    },
    error::ErrorInfo,
    navkey,
    view::ActionItemWidget,
};

#[derive(Debug)]
pub struct ActionMenuState {
    pub list_state: ListState,
    pub is_showing: bool,
    pub target_story_id: Option<arc_core::StoryId>,
}

impl Default for ActionMenuState {
    fn default() -> Self {
        let mut list_state = ListState::default();
        list_state.select(Some(0));

        Self {
            list_state,
            is_showing: false,
            target_story_id: None,
        }
    }
}

pub struct ActionMenu;

impl ActionMenu {
    pub fn window_dimensions() -> (usize, usize) {
        let longest_label_len = ActionMenuItem::ALL
            .iter()
            .map(|item| item.label().len())
            .max()
            .unwrap_or_default();

        let n_labels = ActionMenuItem::ALL.len();

        (longest_label_len + 6, n_labels + 6)
    }
}

impl StatefulWidget for ActionMenu {
    type State = ListState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State)
    where
        Self: Sized,
    {
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .padding(Padding::vertical(2));

        let inner = block.inner(area);

        let highlight_style = Style::default().reversed();

        // Create the list builder
        let builder = ListBuilder::new(move |context| {
            let item = ActionMenuItem::ALL[context.index];
            let widget = ActionItemWidget::new(item.label(), context.is_selected, highlight_style);
            let height = widget.height();

            (widget, height)
        });

        // Create the ListView
        let list = ListView::new(builder, ActionMenuItem::ALL.len());

        let line_len = area.width - 2;
        let line = Line::from("-".repeat(line_len as usize));

        Clear.render(area, buf);
        block.render(area, buf);
        StatefulWidget::render(list, inner, buf, state);

        let top_line_area = Rect::new(area.x + 1, area.y + 1, area.width - 2, 1);
        let bottom_line_area = Rect::new(area.x + 1, area.y + area.height - 2, area.width - 2, 1);

        line.clone().render(top_line_area, buf);
        line.render(bottom_line_area, buf);
    }
}

#[derive(Clone, Copy)]
pub enum ActionMenuItem {
    OpenNote,
    OpenIterationNote,
    EditDescription,
    OpenTmux,
    SetActive,
    CreateGitWorktree,
    OpenInBrowser,
}

impl ActionMenuItem {
    pub const ALL: &[Self] = &[
        Self::OpenNote,
        Self::OpenIterationNote,
        Self::CreateGitWorktree,
        Self::OpenTmux,
        Self::EditDescription,
        Self::SetActive,
        Self::OpenInBrowser,
    ];

    pub fn from_idx(idx: usize) -> ActionMenuItem {
        ActionMenuItem::ALL[idx]
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::OpenNote => "Open Note",
            Self::OpenIterationNote => "Open Iteration Note",
            Self::EditDescription => "Edit Description",
            Self::OpenTmux => "Open Mux Session",
            Self::SetActive => "Toggle Active Story",
            Self::CreateGitWorktree => "Create git worktree",
            Self::OpenInBrowser => "Open ticket in browser",
        }
    }
}

pub fn update(
    ui_state: &mut UiState,
    data_state: &DataState,
    msg: ActionMenuMsg,
    story: &Story,
) -> Vec<Cmd> {
    let item_count = ActionMenuItem::ALL.len();
    let state = &mut ui_state.action_menu.list_state;

    match msg {
        ActionMenuMsg::FocusNext => {
            if item_count == 0 {
                return vec![Cmd::None];
            }
            let current = state.selected.unwrap_or(0);
            let next = if current >= item_count - 1 {
                0 // Wrap to start
            } else {
                current + 1
            };
            state.select(Some(next));
            vec![Cmd::None]
        }

        ActionMenuMsg::FocusPrev => {
            if item_count == 0 {
                return vec![Cmd::None];
            }
            let current = state.selected.unwrap_or(0);
            let prev = if current == 0 {
                item_count - 1 // Wrap to end
            } else {
                current - 1
            };
            state.select(Some(prev));
            vec![Cmd::None]
        }

        ActionMenuMsg::Accept => {
            let mut actions = match ActionMenuItem::from_idx(state.selected.unwrap_or(0)) {
                ActionMenuItem::OpenIterationNote => {
                    let iteration = data_state.current_iterations_ref().and_then(|iterations| {
                        get_story_associated_iteration(story.iteration_id, iterations)
                    });

                    match iteration {
                        Some(it) => vec![Cmd::OpenIterationNote {
                            iteration_id: it.id,
                            iteration_name: it.name.clone(),
                            iteration_app_url: it.app_url.clone(),
                        }],
                        None => {
                            ui_state.errors.push(ErrorInfo::new(
                                "No iteration",
                                "This story has no associated iteration",
                            ));
                            vec![Cmd::None]
                        }
                    }
                }

                ActionMenuItem::OpenNote => {
                    let iteration_app_url = data_state
                        .current_iterations_ref()
                        .and_then(|iterations| {
                            get_story_associated_iteration(story.iteration_id, iterations)
                        })
                        .map(|it| it.app_url.clone());

                    vec![Cmd::OpenNote {
                        story_id: story.id,
                        story_name: story.name.clone(),
                        story_app_url: story.app_url.clone(),
                        iteration_app_url,
                    }]
                }

                ActionMenuItem::EditDescription => {
                    vec![Cmd::EditStoryContent {
                        story_id: story.id,
                        description: story.description.clone(),
                    }]
                }

                ActionMenuItem::OpenTmux => {
                    vec![Cmd::OpenTmuxSession {
                        story_name: story.name.clone(),
                    }]
                }

                ActionMenuItem::SetActive => {
                    vec![Cmd::SelectStory(Some(story.clone())), Cmd::WriteCache]
                }

                ActionMenuItem::CreateGitWorktree => {
                    // TODO: figure out how to get actual branch name
                    vec![Cmd::CreateGitWorktree {
                        branch_name: "feat/branchname".to_string(),
                    }]
                }

                ActionMenuItem::OpenInBrowser => {
                    vec![Cmd::OpenInBrowser {
                        app_url: story.app_url.clone(),
                    }]
                }
            };

            actions.push(Cmd::ActionMenuVisibility(false));
            actions
        }

        ActionMenuMsg::Close => {
            vec![Cmd::ActionMenuVisibility(false)]
        }
    }
}

pub fn key_to_msg(key: KeyEvent) -> Option<ActionMenuMsg> {
    match key.code {
        KeyCode::Enter => Some(ActionMenuMsg::Accept),
        navkey!(down) => Some(ActionMenuMsg::FocusNext),
        navkey!(up) => Some(ActionMenuMsg::FocusPrev),
        KeyCode::Esc | KeyCode::Char('q') => Some(ActionMenuMsg::Close),
        _ => None,
    }
}
