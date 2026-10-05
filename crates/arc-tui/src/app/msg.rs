use crossterm::event::KeyEvent;

use crate::api::{epic::EpicSlim, iteration::Iteration, story::Story};
use crate::app::model::ViewType;
use crate::error::ErrorInfo;
use crate::todos::Todo;

#[derive(Debug, Clone)]
pub enum Msg {
    KeyPressed(KeyEvent),
    StoryList(StoryListMsg),
    NotesList(NotesListMsg),
    IterationList(IterationListMsg),
    EpicList(EpicListMsg),
    TodosList(TodosListMsg),
    ActionMenu(ActionMenuMsg),
    DescriptionModal(DescriptionModalMsg),
    CreateNoteModal(CreateNoteModalMsg),
    AddTodoModal(AddTodoModalMsg),
    StoriesLoaded(Vec<Story>),
    EpicsLoaded(Vec<EpicSlim>),
    IterationsLoaded(Vec<Iteration>),
    AllIterationsLoaded(Vec<Iteration>),
    SwitchToView(ViewType),
    NoteOpened,
    OpenActionMenu,
    OpenKeybindsPanel,
    CacheWritten,
    TodosReloaded(Vec<Todo>),
    Error(ErrorInfo),
    Quit,
}

#[derive(Debug, Clone, Copy)]
pub enum StoryListMsg {
    FocusNext,
    FocusPrev,
    OpenNote,
    SelectStory,
    TmuxEnter,
    EditStoryContents,
    OpenInBrowser,
    ToggleFinished,
}

#[derive(Debug, Clone, Copy)]
pub enum ActionMenuMsg {
    FocusNext,
    FocusPrev,
    Accept,
    Close,
}

#[derive(Debug, Clone, Copy)]
pub enum NotesListMsg {
    FocusNext,
    FocusPrev,
    FocusSectionNext,
    FocusSectionPrev,
    OpenNote,
}

pub use crate::app::pane::searchable_list::SearchListMsg as IterationListMsg;
pub use crate::app::pane::searchable_list::SearchListMsg as EpicListMsg;

#[derive(Debug, Clone)]
pub enum DescriptionModalMsg {
    Close,
    ScrollUp,
    ScrollDown,
    ScrollPageUp,
    ScrollPageDown,
    ScrollToTop,
    ScrollToBottom,
}

#[derive(Debug, Clone)]
pub enum CreateNoteModalMsg {
    Open,
    Close,
    TextInput(char),
    TextBackspace,
    Accept,
}

#[derive(Debug, Clone, Copy)]
pub enum TodosListMsg {
    FocusNext,
    FocusPrev,
    FocusSectionNext,
    FocusSectionPrev,
    ToggleComplete,
    DeleteSelected,
}

#[derive(Debug, Clone)]
pub enum AddTodoModalMsg {
    Open,
    Close,
    TextInput(char),
    TextBackspace,
    Accept,
}
