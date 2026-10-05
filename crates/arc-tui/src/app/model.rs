use throbber_widgets_tui::ThrobberState;
use tokio::task::JoinHandle;
use tui_scrollview::ScrollViewState;

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

use uuid::Uuid;

use crate::{
    api::{epic::EpicSlim, iteration::Iteration, story::Story},
    app::pane::action_menu::ActionMenuState,
    cache::Cache,
    error::ErrorInfo,
    todos::Todo,
};
use arc_core::{Config, EpicId, IterationId, StoryId};

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
    // NOTE: keep the ordering of the below three items consistent with eachother
    pub const ALL: &[Self] = &[
        ViewType::Stories,
        ViewType::Iterations,
        ViewType::Notes,
        ViewType::Todos,
        ViewType::Epics,
        ViewType::Search,
    ];

    pub fn next(self) -> Self {
        match self {
            ViewType::Stories => ViewType::Iterations,
            ViewType::Iterations => ViewType::Notes,
            ViewType::Notes => ViewType::Todos,
            ViewType::Todos => ViewType::Epics,
            ViewType::Epics => ViewType::Search,
            ViewType::Search => ViewType::Stories,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            ViewType::Stories => ViewType::Search,
            ViewType::Search => ViewType::Epics,
            ViewType::Epics => ViewType::Todos,
            ViewType::Todos => ViewType::Notes,
            ViewType::Notes => ViewType::Iterations,
            ViewType::Iterations => ViewType::Stories,
        }
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
    pub stories: Vec<Story>,
    pub iterations: Vec<Iteration>,
    pub epics: Vec<EpicSlim>,
    pub current_iterations: Option<Vec<Iteration>>,
    pub active_story: Option<Story>,
    pub async_handles: Vec<JoinHandle<()>>,
    pub todos: Vec<Todo>,
}

#[derive(Default, Debug)]
pub struct DescriptionModalState {
    pub is_showing: bool,
    pub scroll_view_state: ScrollViewState,
    pub story: Option<Story>,
}

#[derive(Default, Debug)]
pub struct UiState {
    pub active_view: ViewType,
    pub story_list: StoryListState,
    pub notes_list: NotesListState,
    pub iteration_list: IterationListState,
    pub epic_list: EpicListState,
    pub todos_list: TodosListState,
    pub action_menu: ActionMenuState,
    pub description_modal: DescriptionModalState,
    pub create_note_modal: CreateNoteModalState,
    pub add_todo_modal: AddTodoModalState,
    pub show_keybinds_panel: bool,
    pub errors: Vec<ErrorInfo>,
    pub loading: LoadingState,
    pub throbber_state: ThrobberState,
}

// Epic and iteration panes share the same fuzzy-searchable list state.
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
    pub selected_path: Option<PathBuf>,
    pub daily_notes: Vec<PathBuf>,
    pub story_notes: Vec<PathBuf>,
    pub iteration_notes: Vec<PathBuf>,
    pub epic_notes: Vec<PathBuf>,
    pub scratch_notes: Vec<PathBuf>,
    /// Saved selection per section (keyed by section index 0–4).
    pub section_selections: HashMap<usize, PathBuf>,
}

#[derive(Clone, Debug, Default)]
pub struct CreateNoteModalState {
    pub is_showing: bool,
    pub input: String,
}

#[derive(Clone, Debug, Default)]
pub struct AddTodoModalState {
    pub is_showing: bool,
    pub input: String,
}

#[derive(Clone, Debug, Default)]
pub struct TodosListState {
    pub selected_id: Option<Uuid>,
    /// Ids of todos to keep rendering even once completed: every todo that was
    /// incomplete when the TUI opened, has appeared since, or was completed
    /// during this session. Completed todos absent from this set (e.g. note
    /// checkboxes ticked off in a past session) are hidden. Reset each launch.
    pub visible: HashSet<Uuid>,
}

impl TodosListState {
    /// Record every currently-incomplete todo as visible and drop ids that no
    /// longer exist. Call whenever the todo list is loaded or reloaded.
    pub fn refresh_visible(&mut self, todos: &[Todo]) {
        self.visible.retain(|id| todos.iter().any(|t| &t.id == id));
        for todo in todos.iter().filter(|t| !t.completed) {
            self.visible.insert(todo.id);
        }
    }

    /// Whether this todo should be rendered and navigable.
    pub fn is_visible(&self, todo: &Todo) -> bool {
        !todo.completed || self.visible.contains(&todo.id)
    }
}

impl DataState {
    pub fn current_iterations_ref(&self) -> Option<Vec<&Iteration>> {
        self.current_iterations.as_ref().map(|v| v.iter().collect())
    }
}

impl Model {
    pub fn from_cache_and_config(cache: Cache, config: Config, todos: Vec<Todo>) -> Model {
        let mut model = Model {
            data: DataState {
                stories: cache.iteration_stories.clone().unwrap_or_default(),
                epics: cache.epics.clone(),
                current_iterations: cache.current_iterations.clone(),
                active_story: cache.active_story.clone(),
                async_handles: Vec::new(),
                iterations: cache.iterations.clone(),
                todos,
            },
            ui: UiState::default(),
            config,
            cache,
        };
        model.ui.story_list.selected_story_id = model.data.stories.first().map(|s| s.id);
        model.ui.epic_list.selected_id = model.data.epics.first().map(|e| e.id);
        model.ui.todos_list.refresh_visible(&model.data.todos);
        model
    }
}
