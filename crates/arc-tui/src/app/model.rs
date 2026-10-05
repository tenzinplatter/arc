use throbber_widgets_tui::ThrobberState;
use tokio::task::JoinHandle;
use tui_scrollview::ScrollViewState;
use tui_widget_list::ListState;

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

use uuid::Uuid;

use crate::{
    api::{
        epic::EpicSlim,
        iteration::Iteration,
        story::{Story, iteration_of},
    },
    cache::Cache,
    error::ErrorInfo,
    todos::Todo,
};
use arc_core::{Config, EpicId, IterationId, StoryId};
use arc_notes::NoteDir;

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
    pub stories: Vec<Story>,
    pub iterations: Vec<Iteration>,
    pub epics: Vec<EpicSlim>,
    pub current_iterations: Option<Vec<Iteration>>,
    pub active_story: Option<Story>,
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
    pub selected_path: Option<PathBuf>,
    pub notes: HashMap<NoteDir, Vec<PathBuf>>,
    pub section_selections: HashMap<NoteDir, PathBuf>,
}

#[derive(Clone, Debug, Default)]
pub struct TodosListState {
    pub selected_id: Option<Uuid>,
    pub visible: HashSet<Uuid>,
}

impl TodosListState {
    pub fn refresh_visible(&mut self, todos: &[Todo]) {
        self.visible.retain(|id| todos.iter().any(|t| &t.id == id));
        for todo in todos.iter().filter(|t| !t.completed) {
            self.visible.insert(todo.id);
        }
    }

    pub fn is_visible(&self, todo: &Todo) -> bool {
        !todo.completed || self.visible.contains(&todo.id)
    }
}

impl DataState {
    pub fn iteration_for(&self, story: &Story) -> Option<&Iteration> {
        iteration_of(story, self.current_iterations.as_deref()?)
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
