use crossterm::event::{KeyCode, KeyEvent};

use crate::{
    api::story::get_story_associated_iteration,
    app::{
        App,
        cmd::Cmd,
        model::{LoadingState, ViewType},
        msg::{AddTodoModalMsg, CreateNoteModalMsg, EpicListMsg, IterationListMsg, Msg},
        pane::{action_menu, add_todo_modal, create_note_modal, description_modal, epic_list, iteration_list, notes_list, searchable_list, story_list, todos_list},
    },
    error::ErrorInfo,
    keybindings::Key,
};
use arc_core::dbg_file;

impl App {
    pub fn update(
        &mut self,
        msg: Msg,
    ) -> Vec<Cmd> {
        dbg_file!("Update: {:?}", msg);

        match msg {
            Msg::Quit => {
                self.exit = true;
                for handle in &self.model.data.async_handles {
                    if !handle.is_finished() {
                        handle.abort();
                    }
                }
                // Clean up completed todos on exit
                self.model.data.todos.retain(|t| !t.completed);
                vec![Cmd::WriteTodos]
            }

            Msg::KeyPressed(key_event) => self.handle_key_input(key_event),

            Msg::StoryList(story_msg) => story_list::update(
                &mut self.model.ui.story_list,
                &self.model.data.stories,
                self.model.data.current_iterations_ref(),
                story_msg,
            ),

            Msg::NotesList(notes_msg) => notes_list::update(
                &mut self.model.ui.notes_list,
                notes_msg,
            ),

            Msg::IterationList(msg) => {
                let current = self.model.data.current_iterations.as_deref().unwrap_or(&[]);
                let all = &self.model.data.iterations;
                iteration_list::update(&mut self.model.ui.iteration_list, current, all, msg)
            }

            Msg::EpicList(msg) => {
                epic_list::update(&mut self.model.ui.epic_list, &self.model.data.epics, msg)
            }

            Msg::StoriesLoaded {
                stories,
                from_cache,
            } => self.on_stories_loaded(stories, from_cache),

            Msg::EpicsLoaded(epics) => self.on_epics_loaded(epics),

            Msg::IterationsLoaded(mut iterations) => {
                iterations.sort_by_key(|it| std::cmp::Reverse(it.start_date));
                if self.model.ui.iteration_list.selected_id.is_none() {
                    self.model.ui.iteration_list.selected_id = iterations.first().map(|it| it.id);
                }
                self.model.data.current_iterations = Some(iterations.clone());
                self.model.cache.current_iterations = Some(iterations.clone());
                self.model.ui.loading = LoadingState::FetchingStories;

                dbg_file!(
                    "Got iterations: {:?}",
                    iterations.iter().map(|it| it.id).collect::<Vec<_>>()
                );
                vec![
                    Cmd::WriteCache,
                    Cmd::FetchStories {
                        iteration_ids: iterations.iter().map(|it| it.id).collect(),
                    },
                ]
            }

            Msg::AllIterationsLoaded(mut iterations) => {
                iterations.sort_by_key(|it| std::cmp::Reverse(it.start_date));
                self.model.data.iterations = iterations.clone();
                self.model.cache.iterations = iterations;
                vec![Cmd::WriteCache]
            }

            Msg::SwitchToView(view_type) => {
                self.model.ui.active_view = view_type;
                if view_type == ViewType::Notes {
                    let scan = notes_list::scan_notes(&self.model.config.notes_dir);
                    self.model.ui.notes_list.daily_notes = scan.daily;
                    self.model.ui.notes_list.story_notes = scan.stories;
                    self.model.ui.notes_list.iteration_notes = scan.iterations;
                    self.model.ui.notes_list.epic_notes = scan.epics;
                    self.model.ui.notes_list.scratch_notes = scan.scratch;
                    if self.model.ui.notes_list.selected_path.is_none() {
                        let nl = &self.model.ui.notes_list;
                        let first = nl.daily_notes.first()
                            .or_else(|| nl.story_notes.first())
                            .or_else(|| nl.iteration_notes.first())
                            .or_else(|| nl.epic_notes.first())
                            .or_else(|| nl.scratch_notes.first())
                            .cloned();
                        self.model.ui.notes_list.selected_path = first;
                    }
                }
                vec![Cmd::None]
            }

            Msg::NoteOpened => {
                if self.model.ui.active_view == ViewType::Notes {
                    let scan = notes_list::scan_notes(&self.model.config.notes_dir);
                    self.model.ui.notes_list.daily_notes = scan.daily;
                    self.model.ui.notes_list.story_notes = scan.stories;
                    self.model.ui.notes_list.iteration_notes = scan.iterations;
                    self.model.ui.notes_list.epic_notes = scan.epics;
                    self.model.ui.notes_list.scratch_notes = scan.scratch;
                }
                vec![Cmd::None]
            }

            Msg::CacheWritten => vec![Cmd::None],

            Msg::TodosReloaded(todos) => {
                let prev_selected = self.model.ui.todos_list.selected_id;
                self.model.data.todos = todos;
                // Keep already-shown todos visible and pick up any new incomplete
                // ones; completed todos we've never shown stay hidden.
                self.model
                    .ui
                    .todos_list
                    .refresh_visible(&self.model.data.todos);
                // Preserve selection if the UUID still exists; else clear it.
                if let Some(id) = prev_selected
                    && !self.model.data.todos.iter().any(|t| t.id == id)
                {
                    self.model.ui.todos_list.selected_id = None;
                }
                vec![Cmd::None]
            }

            Msg::Error(e) => {
                self.model.ui.errors.push(e);
                // Stop loading spinner on error
                self.model.ui.loading = LoadingState::Loaded;
                vec![Cmd::None]
            }

            Msg::ActionMenu(menu_msg) => {
                let story = self
                    .model
                    .ui
                    .action_menu
                    .target_story_id
                    .and_then(|id| self.model.data.stories.iter().find(|s| s.id == id));

                if let Some(hovered_story) = story {
                    action_menu::update(
                        &mut self.model.ui,
                        &self.model.data,
                        menu_msg,
                        hovered_story,
                    )
                } else {
                    // Target story no longer exists, close menu
                    self.model.ui.action_menu.is_showing = false;
                    vec![Cmd::None]
                }
            }

            Msg::ToggleActionMenu => {
                vec![Cmd::ActionMenuVisibility(
                    !self.model.ui.action_menu.is_showing,
                )]
            }

            Msg::ToggleKeybindsPanel => {
                self.model.ui.show_keybinds_panel = !self.model.ui.show_keybinds_panel;
                vec![Cmd::None]
            }

            Msg::DescriptionModal(modal_msg) => {
                description_modal::update(&mut self.model.ui.description_modal, modal_msg)
            }

            Msg::CreateNoteModal(modal_msg) => create_note_modal::update(
                &mut self.model.ui.create_note_modal,
                &self.model.config,
                modal_msg,
            ),

            Msg::TodosList(msg) => todos_list::update(
                &mut self.model.ui.todos_list,
                &mut self.model.data.todos,
                msg,
            ),

            Msg::AddTodoModal(msg) => add_todo_modal::update(
                &mut self.model.ui.add_todo_modal,
                &mut self.model.data.todos,
                msg,
            ),
        }
    }

    /// Intercepts keys for search state in Iteration/Epic views.
    ///
    /// Two modes:
    /// - **Active** (`search_active = true`): typing mode. j/k are consumed (not
    ///   navigation), Enter passes through, Esc deactivates but keeps the query.
    /// - **Inactive with query** (`search_active = false`, query non-empty): list is
    ///   filtered but navigation works normally. Esc clears the query entirely.
    fn try_handle_search_key(&mut self, key: KeyEvent) -> Option<Vec<Cmd>> {
        let state = match self.model.ui.active_view {
            ViewType::Iterations => &self.model.ui.iteration_list,
            ViewType::Epics => &self.model.ui.epic_list,
            _ => return None,
        };

        match searchable_list::search_key(key.code, state.search_active, !state.search_query.is_empty()) {
            searchable_list::SearchKey::Passthrough => None,
            searchable_list::SearchKey::Consume => Some(vec![Cmd::None]),
            searchable_list::SearchKey::Msg(list_msg) => {
                let msg = match self.model.ui.active_view {
                    ViewType::Iterations => Msg::IterationList(list_msg),
                    ViewType::Epics => Msg::EpicList(list_msg),
                    _ => return None,
                };
                Some(self.update(msg))
            }
        }
    }

    fn handle_key_input(&mut self, key: KeyEvent) -> Vec<Cmd> {
        // Keybinds panel takes highest priority
        if self.model.ui.show_keybinds_panel {
            if matches!(key.code, KeyCode::Char('?') | KeyCode::Char('q') | KeyCode::Esc) {
                self.model.ui.show_keybinds_panel = false;
            }
            return vec![Cmd::None];
        }

        // Description modal takes priority (rendered on top of everything)
        if self.model.ui.description_modal.is_showing {
            return if let Some(msg) = description_modal::key_to_msg(key) {
                self.update(Msg::DescriptionModal(msg))
            } else {
                vec![Cmd::None]
            };
        }

        // action menu is rendered as an overlay, so swallows all keybinds when showing
        if self.model.ui.action_menu.is_showing {
            return if let Some(msg) = action_menu::key_to_msg(key) {
                self.update(Msg::ActionMenu(msg))
            } else {
                vec![Cmd::None]
            };
        }

        // Create note modal intercepts all keys when showing
        if self.model.ui.create_note_modal.is_showing {
            return if let Some(modal_msg) = create_note_modal::key_to_msg(key) {
                self.update(Msg::CreateNoteModal(modal_msg))
            } else {
                vec![Cmd::None]
            };
        }

        // Add todo modal intercepts all keys when showing
        if self.model.ui.add_todo_modal.is_showing {
            return if let Some(msg) = add_todo_modal::key_to_msg(key) {
                self.update(Msg::AddTodoModal(msg))
            } else {
                vec![Cmd::None]
            };
        }

        // Search bar intercepts most keys when active in Iteration/Epic views
        if let Some(cmds) = self.try_handle_search_key(key) {
            return cmds;
        }

        let app_key = Key::from_key_event(key);

        // Global keys
        if let Some(app_key) = app_key {
            match app_key {
                Key::Quit => return self.update(Msg::Quit),
                Key::HelpPanel => return self.update(Msg::ToggleKeybindsPanel),
                Key::ViewNext => {
                    let next = self.model.ui.active_view.next();
                    return self.update(Msg::SwitchToView(next));
                }
                Key::ViewPrev => {
                    let prev = self.model.ui.active_view.prev();
                    return self.update(Msg::SwitchToView(prev));
                }
                Key::DailyNote => {
                    let today = arc_core::time::today();
                    let path = self.config.notes_dir.join("daily").join(format!("{}.md", today));
                    return vec![Cmd::OpenDailyNote { path }];
                }
                _ => {}
            }
        }

        // Route to active view's key handler
        match self.model.ui.active_view {
            ViewType::Iterations => {
                if key.code == KeyCode::Char('/') {
                    return self.update(Msg::IterationList(IterationListMsg::ActivateSearch));
                }
                if let Some(msg) = iteration_list::key_to_msg(key) {
                    return self.update(Msg::IterationList(msg));
                }
            }
            ViewType::Epics => {
                if key.code == KeyCode::Char('/') {
                    return self.update(Msg::EpicList(EpicListMsg::ActivateSearch));
                }
                if let Some(msg) = epic_list::key_to_msg(key) {
                    return self.update(Msg::EpicList(msg));
                }
            }
            ViewType::Stories => {
                if key.code == KeyCode::Enter {
                    return self.update(Msg::ToggleActionMenu);
                }

                if let Some(app_key) = app_key {
                    match app_key {
                        Key::Description => {
                            let story = self
                                .model
                                .ui
                                .story_list
                                .selected_story_id
                                .and_then(|id| self.model.data.stories.iter().find(|s| s.id == id));

                            if let Some(story) = story {
                                description_modal::open(
                                    &mut self.model.ui.description_modal,
                                    story.clone(),
                                );
                            }
                            return vec![Cmd::None];
                        }
                        Key::IterationNote => {
                            let story = self
                                .model
                                .ui
                                .story_list
                                .selected_story_id
                                .and_then(|id| self.model.data.stories.iter().find(|s| s.id == id));

                            if let Some(story) = story {
                                let iteration = self
                                    .model
                                    .data
                                    .current_iterations_ref()
                                    .and_then(|its| {
                                        get_story_associated_iteration(story.iteration_id, its)
                                    });

                                return match iteration {
                                    Some(it) => vec![Cmd::OpenIterationNote {
                                        iteration_id: it.id,
                                        iteration_name: it.name.clone(),
                                        iteration_app_url: it.app_url.clone(),
                                    }],
                                    None => {
                                        self.model.ui.errors.push(ErrorInfo::new(
                                            "No iteration",
                                            "This story has no associated iteration",
                                        ));
                                        vec![Cmd::None]
                                    }
                                };
                            }
                            return vec![Cmd::None];
                        }
                        _ => {}
                    }
                }

                if let Some(msg) = story_list::key_to_msg(key) {
                    return self.update(Msg::StoryList(msg));
                }
            }
            ViewType::Notes => {
                if key.code == KeyCode::Char('n') {
                    return self.update(Msg::CreateNoteModal(CreateNoteModalMsg::Open));
                }
                if let Some(msg) = notes_list::key_to_msg(key) {
                    return self.update(Msg::NotesList(msg));
                }
            }
            ViewType::Todos => {
                if key.code == KeyCode::Char('n') {
                    // On a note-parsed todo, open its source note; otherwise add a todo.
                    let source = self
                        .model
                        .ui
                        .todos_list
                        .selected_id
                        .and_then(|id| self.model.data.todos.iter().find(|t| t.id == id))
                        .map(|t| &t.source);
                    if let Some(crate::todos::TodoSource::NoteParsed { file, .. }) = source {
                        let path = self.config.notes_dir.join(file);
                        return vec![Cmd::OpenDailyNote { path }];
                    }
                    return self.update(Msg::AddTodoModal(AddTodoModalMsg::Open));
                }
                if let Some(msg) = todos_list::key_to_msg(key) {
                    return self.update(Msg::TodosList(msg));
                }
            }
            _ => {}
        }

        vec![Cmd::None]
    }

    /// Reconcile freshly-loaded stories against UI state (selection, open modals,
    /// active story) and persist. Returns the follow-up commands.
    fn on_stories_loaded(&mut self, stories: Vec<crate::api::story::Story>, from_cache: bool) -> Vec<Cmd> {
        // Only transition to Loaded on fresh API data, not cached — this keeps the
        // spinner showing during a background refresh.
        if !from_cache {
            self.model.ui.loading = LoadingState::Loaded;
        }

        if self.model.ui.story_list.selected_story_id.is_none() && !stories.is_empty() {
            self.model.ui.story_list.selected_story_id = stories.first().map(|s| s.id);
        }

        if !from_cache && id_set_unchanged(&self.model.data.stories, &stories, |s| s.id) {
            return vec![Cmd::None];
        }

        self.model.data.stories = stories.clone();

        // Reconcile selection: if selected story no longer exists, select first.
        if let Some(selected_id) = self.model.ui.story_list.selected_story_id
            && !stories.iter().any(|s| s.id == selected_id)
        {
            self.model.ui.story_list.selected_story_id = stories.first().map(|s| s.id);
        }

        // Reconcile description modal against fresh data.
        if self.model.ui.description_modal.is_showing
            && let Some(ref modal_story) = self.model.ui.description_modal.story
        {
            match stories.iter().find(|s| s.id == modal_story.id) {
                Some(fresh_story) => {
                    self.model.ui.description_modal.story = Some(fresh_story.clone());
                }
                None => {
                    self.model.ui.description_modal.is_showing = false;
                    self.model.ui.description_modal.scroll_view_state = Default::default();
                    self.model.ui.description_modal.story = None;
                    self.model.ui.errors.push(ErrorInfo::new(
                        "Story no longer available".to_string(),
                        "The story was removed or moved out of this iteration".to_string(),
                    ));
                }
            }
        }

        // Reconcile action menu.
        if self.model.ui.action_menu.is_showing
            && let Some(target_id) = self.model.ui.action_menu.target_story_id
            && !stories.iter().any(|s| s.id == target_id)
        {
            self.model.ui.action_menu.is_showing = false;
            self.model.ui.action_menu.target_story_id = None;
            self.model.ui.errors.push(ErrorInfo::new(
                "Story no longer available".to_string(),
                "The story was removed or moved out of this iteration".to_string(),
            ));
        }

        // Reconcile active story.
        if let Some(ref active) = self.model.data.active_story
            && active_story_left_iteration(
                active,
                &stories,
                self.model.data.current_iterations.as_deref().unwrap_or(&[]),
                self.model.cache.user_id,
            )
        {
            self.model.data.active_story = None;
            self.model.cache.active_story = None;
            self.model.ui.errors.push(ErrorInfo::new(
                "Active story cleared".to_string(),
                "The active story is no longer in the current iteration".to_string(),
            ));
        }

        self.model.cache.iteration_stories = Some(stories);
        vec![Cmd::WriteCache]
    }

    fn on_epics_loaded(&mut self, mut epics: Vec<crate::api::epic::EpicSlim>) -> Vec<Cmd> {
        epics.sort_by_key(|e| std::cmp::Reverse(e.created_at));

        if id_set_unchanged(&self.model.data.epics, &epics, |e| e.id) {
            return vec![Cmd::None];
        }

        if self.model.ui.epic_list.selected_id.is_none() {
            self.model.ui.epic_list.selected_id = epics.first().map(|e| e.id);
        }
        self.model.data.epics = epics.clone();
        self.model.cache.epics = epics;
        vec![Cmd::WriteCache]
    }
}

/// True only when the active story was one of our owned current-iteration
/// stories and no longer shows up in that list. Stories set from outside the
/// iteration (or owned by someone else) are left alone.
fn active_story_left_iteration(
    active: &crate::api::story::Story,
    stories: &[crate::api::story::Story],
    current_iterations: &[crate::api::iteration::Iteration],
    user_id: Option<uuid::Uuid>,
) -> bool {
    !stories.iter().any(|s| s.id == active.id)
        && active
            .iteration_id
            .is_some_and(|id| current_iterations.iter().any(|it| it.id == id))
        && user_id.is_some_and(|uid| active.owner_ids.contains(&uid))
}

/// True when both slices hold the same ids in the same order — used to skip a
/// re-render/cache-write when a refresh returned an unchanged list.
fn id_set_unchanged<T>(old: &[T], new: &[T], id: impl Fn(&T) -> i32) -> bool {
    old.len() == new.len() && old.iter().zip(new).all(|(a, b)| id(a) == id(b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{
        iteration::{Iteration, IterationStatus},
        story::Story,
    };
    use chrono::NaiveDate;
    use uuid::Uuid;

    fn iteration(id: i32) -> Iteration {
        let date = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        Iteration {
            id,
            name: String::new(),
            start_date: date,
            end_date: date,
            app_url: String::new(),
            status: IterationStatus::Started,
        }
    }

    fn story(id: i32, iteration_id: Option<i32>, owner_ids: Vec<Uuid>) -> Story {
        Story {
            id,
            iteration_id,
            owner_ids,
            ..Default::default()
        }
    }

    #[test]
    fn active_story_retention() {
        let me = Uuid::from_u128(1);
        let other = Uuid::from_u128(2);
        let current = [iteration(10)];
        let listed = [story(1, Some(10), vec![me])];

        let still_listed = story(1, Some(10), vec![me]);
        assert!(!active_story_left_iteration(&still_listed, &listed, &current, Some(me)));

        let dropped_out = story(2, Some(10), vec![me]);
        assert!(active_story_left_iteration(&dropped_out, &listed, &current, Some(me)));

        let other_iteration = story(3, Some(99), vec![me]);
        assert!(!active_story_left_iteration(&other_iteration, &listed, &current, Some(me)));

        let no_iteration = story(4, None, vec![me]);
        assert!(!active_story_left_iteration(&no_iteration, &listed, &current, Some(me)));

        let someone_elses = story(5, Some(10), vec![other]);
        assert!(!active_story_left_iteration(&someone_elses, &listed, &current, Some(me)));

        let old_cache_copy = story(6, Some(10), vec![]);
        assert!(!active_story_left_iteration(&old_cache_copy, &listed, &current, Some(me)));

        assert!(!active_story_left_iteration(&dropped_out, &listed, &current, None));
    }
}
