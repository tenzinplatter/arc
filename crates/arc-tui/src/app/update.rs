use crossterm::event::{KeyCode, KeyEvent};

use crate::{
    app::{
        App,
        cmd::{Cmd, NoteToOpen},
        model::{CacheView, LoadingState, Overlay, ViewType},
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
                self.model.data.todos.retain(|t| !t.completed);
                vec![Cmd::WriteTodos]
            }

            Msg::KeyPressed(key_event) => self.handle_key_input(key_event),

            Msg::StoryList(story_msg) => story_list::update(
                &mut self.model.ui.story_list,
                self.model.cache.stories(),
                self.model.cache.current_iterations(),
                story_msg,
            ),

            Msg::NotesList(notes_msg) => notes_list::update(
                &mut self.model.ui.notes_list,
                notes_msg,
            ),

            Msg::IterationList(msg) => iteration_list::update(
                &mut self.model.ui.iteration_list,
                self.model.cache.current_iterations(),
                &self.model.cache.iterations,
                msg,
            ),

            Msg::EpicList(msg) => {
                epic_list::update(&mut self.model.ui.epic_list, &self.model.cache.epics, msg)
            }

            Msg::StoriesLoaded(stories) => self.on_stories_loaded(stories),

            Msg::EpicsLoaded(epics) => self.on_epics_loaded(epics),

            Msg::IterationsLoaded(mut iterations) => {
                iterations.sort_by_key(|it| std::cmp::Reverse(it.start_date));
                if self.model.ui.iteration_list.selected_id.is_none() {
                    self.model.ui.iteration_list.selected_id = iterations.first().map(|it| it.id);
                }
                let iteration_ids: Vec<_> = iterations.iter().map(|it| it.id).collect();
                self.model.cache.current_iterations = Some(iterations);
                self.model.ui.loading = LoadingState::FetchingStories;

                dbg_file!("Got iterations: {:?}", iteration_ids);
                vec![Cmd::WriteCache, Cmd::FetchStories { iteration_ids }]
            }

            Msg::AllIterationsLoaded(mut iterations) => {
                iterations.sort_by_key(|it| std::cmp::Reverse(it.start_date));
                self.model.cache.iterations = iterations;
                vec![Cmd::WriteCache]
            }

            Msg::SwitchToView(view_type) => {
                self.model.ui.active_view = view_type;
                if view_type == ViewType::Notes {
                    self.model.ui.notes_list.rescan(&self.model.config.notes_dir);
                    self.model.ui.notes_list.select_first_if_none();
                }
                vec![Cmd::None]
            }

            Msg::NoteOpened => {
                if self.model.ui.active_view == ViewType::Notes {
                    self.model.ui.notes_list.rescan(&self.model.config.notes_dir);
                }
                vec![Cmd::None]
            }

            Msg::CacheWritten => vec![Cmd::None],

            Msg::TodosReloaded(todos) => {
                let prev_selected = self.model.ui.todos_list.selected_id;
                self.model.data.todos = todos;
                self.model
                    .ui
                    .todos_list
                    .refresh_visible(&self.model.data.todos);
                if let Some(id) = prev_selected
                    && !self.model.data.todos.iter().any(|t| t.id == id)
                {
                    self.model.ui.todos_list.selected_id = None;
                }
                vec![Cmd::None]
            }

            Msg::Error(e) => {
                self.model.ui.errors.push(e);
                self.model.ui.loading = LoadingState::Loaded;
                vec![Cmd::None]
            }

            Msg::ActionMenu(menu_msg) => {
                let overlay = &mut self.model.ui.overlay;
                let Overlay::ActionMenu { target, list } = overlay else {
                    return vec![Cmd::None];
                };
                let cache = &self.model.cache;
                match cache.stories().iter().find(|s| s.id == *target) {
                    Some(story) => action_menu::update(list, cache, menu_msg, story),
                    None => {
                        *overlay = Overlay::None;
                        vec![Cmd::None]
                    }
                }
            }

            Msg::OpenActionMenu => {
                if let Some(target) = self.model.ui.story_list.selected_story_id {
                    self.model.ui.overlay = Overlay::action_menu(target);
                }
                vec![Cmd::None]
            }

            Msg::OpenKeybindsPanel => {
                self.model.ui.overlay = Overlay::Keybinds;
                vec![Cmd::None]
            }

            Msg::DescriptionModal(modal_msg) => {
                description_modal::update(&mut self.model.ui.overlay, modal_msg)
            }

            Msg::CreateNoteModal(modal_msg) => {
                create_note_modal::update(&mut self.model.ui.overlay, modal_msg)
            }

            Msg::TodosList(msg) => todos_list::update(
                &mut self.model.ui.todos_list,
                &mut self.model.data.todos,
                msg,
            ),

            Msg::AddTodoModal(msg) => add_todo_modal::update(
                &mut self.model.ui.overlay,
                &mut self.model.data.todos,
                msg,
            ),
        }
    }

    fn try_handle_search_key(&mut self, key: KeyEvent) -> Option<Vec<Cmd>> {
        let search_key = match self.model.ui.active_view {
            ViewType::Iterations => self.model.ui.iteration_list.search_key(key.code),
            ViewType::Epics => self.model.ui.epic_list.search_key(key.code),
            _ => return None,
        };

        match search_key {
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

    fn handle_overlay_key(&mut self, key: KeyEvent) -> Vec<Cmd> {
        let msg = match &self.model.ui.overlay {
            Overlay::None => None,
            Overlay::Keybinds => {
                if matches!(key.code, KeyCode::Char('?') | KeyCode::Char('q') | KeyCode::Esc) {
                    self.model.ui.overlay = Overlay::None;
                }
                None
            }
            Overlay::Description { .. } => {
                description_modal::key_to_msg(key).map(Msg::DescriptionModal)
            }
            Overlay::ActionMenu { .. } => action_menu::key_to_msg(key).map(Msg::ActionMenu),
            Overlay::CreateNote { .. } => {
                create_note_modal::key_to_msg(key).map(Msg::CreateNoteModal)
            }
            Overlay::AddTodo { .. } => add_todo_modal::key_to_msg(key).map(Msg::AddTodoModal),
        };
        match msg {
            Some(msg) => self.update(msg),
            None => vec![Cmd::None],
        }
    }

    fn handle_key_input(&mut self, key: KeyEvent) -> Vec<Cmd> {
        if !matches!(self.model.ui.overlay, Overlay::None) {
            return self.handle_overlay_key(key);
        }

        if let Some(cmds) = self.try_handle_search_key(key) {
            return cmds;
        }

        let app_key = Key::from_key_event(key);

        if let Some(app_key) = app_key {
            match app_key {
                Key::Quit => return self.update(Msg::Quit),
                Key::HelpPanel => return self.update(Msg::OpenKeybindsPanel),
                Key::ViewNext => {
                    let next = self.model.ui.active_view.next();
                    return self.update(Msg::SwitchToView(next));
                }
                Key::ViewPrev => {
                    let prev = self.model.ui.active_view.prev();
                    return self.update(Msg::SwitchToView(prev));
                }
                Key::DailyNote => {
                    return vec![Cmd::OpenNote(NoteToOpen::daily(arc_core::time::today()))];
                }
                _ => {}
            }
        }

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
                    return self.update(Msg::OpenActionMenu);
                }

                if let Some(app_key) = app_key {
                    match app_key {
                        Key::Description => {
                            if let Some(story) = self.selected_story() {
                                self.model.ui.overlay = Overlay::description(story.clone());
                            }
                            return vec![Cmd::None];
                        }
                        Key::IterationNote => {
                            return match self.selected_story() {
                                Some(story) => vec![Cmd::open_iteration_note(
                                    self.model.cache.iteration_for(story),
                                )],
                                None => vec![Cmd::None],
                            };
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
                    let source = self
                        .model
                        .ui
                        .todos_list
                        .selected_id
                        .and_then(|id| self.model.data.todos.iter().find(|t| t.id == id))
                        .map(|t| &t.source);
                    if let Some(crate::todos::TodoSource::NoteParsed { file, .. }) = source {
                        return vec![Cmd::OpenNote(NoteToOpen::existing_or_daily_template(
                            file.clone(),
                        ))];
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

    fn selected_story(&self) -> Option<&crate::api::story::Story> {
        let id = self.model.ui.story_list.selected_story_id?;
        self.model.cache.stories().iter().find(|s| s.id == id)
    }

    fn on_stories_loaded(&mut self, stories: Vec<crate::api::story::Story>) -> Vec<Cmd> {
        self.model.ui.loading = LoadingState::Loaded;

        if self.model.ui.story_list.selected_story_id.is_none() && !stories.is_empty() {
            self.model.ui.story_list.selected_story_id = stories.first().map(|s| s.id);
        }

        if id_set_unchanged(self.model.cache.stories(), &stories, |s| s.id) {
            return vec![Cmd::None];
        }

        if let Some(selected_id) = self.model.ui.story_list.selected_story_id
            && !stories.iter().any(|s| s.id == selected_id)
        {
            self.model.ui.story_list.selected_story_id = stories.first().map(|s| s.id);
        }

        self.reconcile_overlay(&stories);

        if let Some(ref active) = self.model.cache.active_story
            && active_story_left_iteration(
                active,
                &stories,
                self.model.cache.current_iterations(),
                self.api_client.user().id,
            )
        {
            self.model.cache.active_story = None;
            self.model.ui.errors.push(ErrorInfo::new(
                "Active story cleared".to_string(),
                "The active story is no longer in the current iteration".to_string(),
            ));
        }

        self.model.cache.iteration_stories = Some(stories);
        vec![Cmd::WriteCache]
    }

    fn reconcile_overlay(&mut self, stories: &[crate::api::story::Story]) {
        let overlay_story = match &mut self.model.ui.overlay {
            Overlay::Description { story, .. } => story.id,
            Overlay::ActionMenu { target, .. } => *target,
            _ => return,
        };
        match stories.iter().find(|s| s.id == overlay_story) {
            Some(fresh) => {
                if let Overlay::Description { story, .. } = &mut self.model.ui.overlay {
                    *story = fresh.clone();
                }
            }
            None => {
                self.model.ui.overlay = Overlay::None;
                self.model.ui.errors.push(ErrorInfo::new(
                    "Story no longer available".to_string(),
                    "The story was removed or moved out of this iteration".to_string(),
                ));
            }
        }
    }

    fn on_epics_loaded(&mut self, mut epics: Vec<crate::api::epic::EpicSlim>) -> Vec<Cmd> {
        epics.sort_by_key(|e| std::cmp::Reverse(e.created_at));

        if id_set_unchanged(&self.model.cache.epics, &epics, |e| e.id) {
            return vec![Cmd::None];
        }

        if self.model.ui.epic_list.selected_id.is_none() {
            self.model.ui.epic_list.selected_id = epics.first().map(|e| e.id);
        }
        self.model.cache.epics = epics;
        vec![Cmd::WriteCache]
    }
}

fn active_story_left_iteration(
    active: &crate::api::story::Story,
    stories: &[crate::api::story::Story],
    current_iterations: &[crate::api::iteration::Iteration],
    user_id: crate::api::user::MemberId,
) -> bool {
    !stories.iter().any(|s| s.id == active.id)
        && crate::api::story::iteration_of(active, current_iterations).is_some()
        && active.owner_ids.contains(&user_id)
}

fn id_set_unchanged<T, Id: PartialEq>(old: &[T], new: &[T], id: impl Fn(&T) -> Id) -> bool {
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
    use crate::api::user::MemberId;
    use uuid::Uuid;

    fn iteration(id: i32) -> Iteration {
        let date = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        Iteration {
            id: id.into(),
            name: String::new(),
            start_date: date,
            end_date: date,
            app_url: String::new(),
            status: IterationStatus::Started,
        }
    }

    fn story(id: i32, iteration_id: Option<i32>, owner_ids: Vec<MemberId>) -> Story {
        Story {
            id: id.into(),
            iteration_id: iteration_id.map(Into::into),
            owner_ids,
            ..Default::default()
        }
    }

    #[test]
    fn active_story_retention() {
        let me = MemberId::from(Uuid::from_u128(1));
        let other = MemberId::from(Uuid::from_u128(2));
        let current = [iteration(10)];
        let listed = [story(1, Some(10), vec![me])];

        let still_listed = story(1, Some(10), vec![me]);
        assert!(!active_story_left_iteration(&still_listed, &listed, &current, me));

        let dropped_out = story(2, Some(10), vec![me]);
        assert!(active_story_left_iteration(&dropped_out, &listed, &current, me));

        let other_iteration = story(3, Some(99), vec![me]);
        assert!(!active_story_left_iteration(&other_iteration, &listed, &current, me));

        let no_iteration = story(4, None, vec![me]);
        assert!(!active_story_left_iteration(&no_iteration, &listed, &current, me));

        let someone_elses = story(5, Some(10), vec![other]);
        assert!(!active_story_left_iteration(&someone_elses, &listed, &current, me));

        let old_cache_copy = story(6, Some(10), vec![]);
        assert!(!active_story_left_iteration(&old_cache_copy, &listed, &current, me));

    }
}
