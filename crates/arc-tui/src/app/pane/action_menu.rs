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
    api::story::Story,
    app::{
        cmd::{Cmd, NoteToOpen},
        model::DataState,
        msg::ActionMenuMsg,
    },
    error::ErrorInfo,
    navkey,
    view::ActionItemWidget,
};

const PLACEHOLDER_WORKTREE_BRANCH: &str = "feat/branchname";

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

        let builder = ListBuilder::new(move |context| {
            let item = ActionMenuItem::ALL[context.index];
            let widget = ActionItemWidget::new(item.label(), context.is_selected, highlight_style);
            let height = widget.height();

            (widget, height)
        });

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
    list: &mut ListState,
    errors: &mut Vec<ErrorInfo>,
    data_state: &DataState,
    msg: ActionMenuMsg,
    story: &Story,
) -> Vec<Cmd> {
    let item_count = ActionMenuItem::ALL.len();
    let current = list.selected.unwrap_or(0);

    match msg {
        ActionMenuMsg::FocusNext => {
            list.select(Some((current + 1) % item_count));
            vec![Cmd::None]
        }

        ActionMenuMsg::FocusPrev => {
            list.select(Some((current + item_count - 1) % item_count));
            vec![Cmd::None]
        }

        ActionMenuMsg::Accept => {
            let mut actions = match ActionMenuItem::from_idx(current) {
                ActionMenuItem::OpenIterationNote => {
                    match data_state.iteration_for(story) {
                        Some(it) => vec![Cmd::OpenNote(NoteToOpen::iteration(it))],
                        None => {
                            errors.push(ErrorInfo::new(
                                "No iteration",
                                "This story has no associated iteration",
                            ));
                            vec![Cmd::None]
                        }
                    }
                }

                ActionMenuItem::OpenNote => {
                    let iteration_app_url =
                        data_state.iteration_for(story).map(|it| it.app_url.clone());

                    vec![Cmd::OpenNote(NoteToOpen::story(story, iteration_app_url))]
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
                    vec![Cmd::CreateGitWorktree {
                        branch_name: PLACEHOLDER_WORKTREE_BRANCH.to_string(),
                    }]
                }

                ActionMenuItem::OpenInBrowser => {
                    vec![Cmd::OpenInBrowser {
                        app_url: story.app_url.clone(),
                    }]
                }
            };

            actions.push(Cmd::CloseActionMenu);
            actions
        }

        ActionMenuMsg::Close => {
            vec![Cmd::CloseActionMenu]
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
