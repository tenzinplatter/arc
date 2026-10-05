use throbber_widgets_tui::ThrobberState;
use tokio::task::JoinHandle;
use tui_scrollview::ScrollViewState;
use tui_widget_list::ListState;

use std::collections::{HashMap, HashSet};

use uuid::Uuid;

use crate::{
    api::story::Story,
    cache::Cache,
    error::ErrorInfo,
    todos::Todo,
};
use arc_core::{Config, EpicId, IterationId, StoryId};
use arc_notes::{NoteDir, NoteRelPath};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LoadingState {
    #[default]
    FetchingIteration,
    FetchingStories,
    Loaded,
}

impl LoadingState {
    pub fn label(&self) -> &'static str {
        match self {
            LoadingState::FetchingIteration => "Fetching iterations...",
            LoadingState::FetchingStories => "Loading stories...",
            LoadingState::Loaded => "",
        }
    }

    pub fn is_loading(&self) -> bool {
        !matches!(self, LoadingState::Loaded)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewType {
    #[default]
    Stories,
    Epics,
    Notes,
    Todos,
    Search,
    Iterations,
}

impl ViewType {
    pub const ALL: &[Self] = &[
        ViewType::Stories,
        ViewType::Iterations,
        ViewType::Notes,
        ViewType::Todos,
        ViewType::Epics,
        ViewType::Search,
    ];

    pub fn next(self) -> Self {
        Self::ALL[(self.position() + 1) % Self::ALL.len()]
    }

    pub fn prev(self) -> Self {
        Self::ALL[(self.position() + Self::ALL.len() - 1) % Self::ALL.len()]
    }

    fn position(self) -> usize {
        Self::ALL
            .iter()
            .position(|&v| v == self)
            .expect("every ViewType is listed in ALL")
    }

    pub fn label(&self) -> &'static str {
        match self {
            ViewType::Stories => "Stories",
            ViewType::Epics => "Epics",
            ViewType::Notes => "Notes",
            ViewType::Todos => "Todos",
            ViewType::Search => "Search",
            ViewType::Iterations => "Iterations",
        }
    }
}

#[derive(Debug)]
pub struct Model {
    pub data: DataState,
    pub ui: UiState,
    pub config: Config,
    pub cache: Cache,
}

#[derive(Default, Debug)]
pub struct DataState {
    pub async_handles: Vec<JoinHandle<()>>,
    pub todos: Vec<Todo>,
}

#[derive(Debug, Default)]
pub enum Overlay {
    #[default]
    None,
    Keybinds,
    Description {
        story: Story,
        scroll: ScrollViewState,
    },
    ActionMenu {
        target: StoryId,
        list: ListState,
    },
    CreateNote {
        input: String,
    },
    AddTodo {
        input: String,
    },
}

impl Overlay {
    pub fn description(story: Story) -> Self {
        Overlay::Description {
            story,
            scroll: ScrollViewState::default(),
        }
    }

    pub fn action_menu(target: StoryId) -> Self {
        let mut list = ListState::default();
        list.select(Some(0));
        Overlay::ActionMenu { target, list }
    }
}

#[derive(Default, Debug)]
pub struct UiState {
    pub active_view: ViewType,
    pub story_list: StoryListState,
    pub notes_list: NotesListState,
    pub iteration_list: IterationListState,
    pub epic_list: EpicListState,
    pub todos_list: TodosListState,
    pub overlay: Overlay,
    pub errors: Vec<ErrorInfo>,
    pub loading: LoadingState,
    pub throbber_state: ThrobberState,
}

pub type IterationListState = crate::app::pane::searchable_list::SearchListState<IterationId>;
pub type EpicListState = crate::app::pane::searchable_list::SearchListState<EpicId>;

#[derive(Clone, Debug)]
pub struct StoryListState {
    pub selected_story_id: Option<StoryId>,
    pub show_finished: bool,
}

impl Default for StoryListState {
    fn default() -> Self {
        Self {
            selected_story_id: Default::default(),
            show_finished: true,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct NotesListState {
    pub selected_path: Option<NoteRelPath>,
    pub notes: HashMap<NoteDir, Vec<NoteRelPath>>,
    pub section_selections: HashMap<NoteDir, NoteRelPath>,
}

#[derive(Clone, Debug, Default)]
pub struct TodosListState {
    pub selected_id: Option<Uuid>,
    pub visible: HashSet<Uuid>,
}

impl TodosListState {
    pub fn new(todos: &[Todo]) -> Self {
        Self {
            selected_id: None,
            visible: incomplete_ids(todos).collect(),
        }
    }

    pub fn refresh_visible(&mut self, todos: &[Todo]) {
        self.visible.retain(|id| todos.iter().any(|t| &t.id == id));
        self.visible.extend(incomplete_ids(todos));
    }

    pub fn is_visible(&self, todo: &Todo) -> bool {
        !todo.completed || self.visible.contains(&todo.id)
    }
}

fn incomplete_ids(todos: &[Todo]) -> impl Iterator<Item = Uuid> + '_ {
    todos.iter().filter(|t| !t.completed).map(|t| t.id)
}

impl Model {
    pub fn new(
        cache: Cache,
        config: Config,
        todos: Vec<Todo>,
        async_handles: Vec<JoinHandle<()>>,
    ) -> Model {
        let ui = UiState {
            story_list: StoryListState {
                selected_story_id: cache.stories().first().map(|s| s.id),
                ..Default::default()
            },
            epic_list: EpicListState {
                selected_id: cache.epics.first().map(|e| e.id),
                ..Default::default()
            },
            todos_list: TodosListState::new(&todos),
            ..Default::default()
        };
        Model {
            data: DataState {
                async_handles,
                todos,
            },
            ui,
            config,
            cache,
        }
    }
}
