use std::path::Path;

use arc_notes::{NoteDir, NoteRelPath};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::{
    app::{cmd::{Cmd, NoteToOpen}, msg::NotesListMsg},
    navkey,
};

pub use crate::app::model::NotesListState;

fn scan_dir_newest_first(notes_dir: &Path, dir: NoteDir) -> Vec<NoteRelPath> {
    let Ok(entries) = std::fs::read_dir(notes_dir.join(dir)) else {
        return Vec::new();
    };
    let mut notes: Vec<NoteRelPath> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("md"))
        .filter_map(|path| NoteRelPath::from_abs(notes_dir, &path))
        .collect();
    notes.sort_by(|a, b| b.cmp(a));
    notes
}

impl NotesListState {
    pub fn rescan(&mut self, notes_dir: &Path) {
        self.notes = NoteDir::ALL
            .into_iter()
            .map(|dir| (dir, scan_dir_newest_first(notes_dir, dir)))
            .collect();
    }

    pub fn select_first_if_none(&mut self) {
        if self.selected_path.is_none() {
            self.selected_path = self.all_notes().first().map(|p| (*p).clone());
        }
    }

    pub fn notes_in(&self, dir: NoteDir) -> &[NoteRelPath] {
        self.notes.get(&dir).map(Vec::as_slice).unwrap_or_default()
    }

    pub fn sections(&self) -> impl Iterator<Item = (NoteDir, &[NoteRelPath])> {
        NoteDir::ALL.into_iter().map(|dir| (dir, self.notes_in(dir)))
    }

    pub fn is_empty(&self) -> bool {
        self.sections().all(|(_, notes)| notes.is_empty())
    }

    fn all_notes(&self) -> Vec<&NoteRelPath> {
        self.sections().flat_map(|(_, notes)| notes).collect()
    }

    fn section_of(&self, path: &NoteRelPath) -> Option<NoteDir> {
        self.sections()
            .find(|(_, notes)| notes.contains(path))
            .map(|(dir, _)| dir)
    }
}

pub fn update(state: &mut NotesListState, msg: NotesListMsg) -> Vec<Cmd> {
    match msg {
        NotesListMsg::FocusNext => {
            let notes = state.all_notes();
            if notes.is_empty() {
                return vec![Cmd::None];
            }

            let current_idx = state
                .selected_path
                .as_ref()
                .and_then(|sel| notes.iter().position(|p| *p == sel));

            let next_idx = match current_idx {
                Some(idx) => (idx + 1) % notes.len(),
                None => 0,
            };

            state.selected_path = Some(notes[next_idx].clone());
            vec![Cmd::None]
        }

        NotesListMsg::FocusPrev => {
            let notes = state.all_notes();
            if notes.is_empty() {
                return vec![Cmd::None];
            }

            let current_idx = state
                .selected_path
                .as_ref()
                .and_then(|sel| notes.iter().position(|p| *p == sel));

            let prev_idx = match current_idx {
                Some(0) | None => notes.len() - 1,
                Some(idx) => idx - 1,
            };

            state.selected_path = Some(notes[prev_idx].clone());
            vec![Cmd::None]
        }

        NotesListMsg::FocusSectionNext | NotesListMsg::FocusSectionPrev => {
            let non_empty: Vec<NoteDir> = state
                .sections()
                .filter(|(_, notes)| !notes.is_empty())
                .map(|(dir, _)| dir)
                .collect();
            if non_empty.is_empty() {
                return vec![Cmd::None];
            }

            let cur_section = state
                .selected_path
                .as_ref()
                .and_then(|sel| state.section_of(sel));
            if let (Some(sel), Some(dir)) = (state.selected_path.clone(), cur_section) {
                state.section_selections.insert(dir, sel);
            }

            let cur_pos = cur_section
                .and_then(|dir| non_empty.iter().position(|&d| d == dir))
                .unwrap_or(0);
            let next_pos = if matches!(msg, NotesListMsg::FocusSectionNext) {
                (cur_pos + 1) % non_empty.len()
            } else {
                (cur_pos + non_empty.len() - 1) % non_empty.len()
            };
            let target = non_empty[next_pos];

            let target_notes = state.notes_in(target);
            state.selected_path = state
                .section_selections
                .get(&target)
                .filter(|p| target_notes.contains(p))
                .or_else(|| target_notes.first())
                .cloned();

            vec![Cmd::None]
        }

        NotesListMsg::OpenNote => {
            if let Some(ref path) = state.selected_path {
                vec![Cmd::OpenNote(NoteToOpen::existing_or_daily_template(path.clone()))]
            } else {
                vec![Cmd::None]
            }
        }
    }
}

pub fn key_to_msg(key: KeyEvent) -> Option<NotesListMsg> {
    match key.code {
        KeyCode::Char('j') if key.modifiers == KeyModifiers::CONTROL => {
            Some(NotesListMsg::FocusSectionNext)
        }
        KeyCode::Char('k') if key.modifiers == KeyModifiers::CONTROL => {
            Some(NotesListMsg::FocusSectionPrev)
        }
        navkey!(down) => Some(NotesListMsg::FocusNext),
        navkey!(up) => Some(NotesListMsg::FocusPrev),
        KeyCode::Enter => Some(NotesListMsg::OpenNote),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_jump_wraps_in_display_order_and_restores_last_selection() {
        let rel = |p: &str| NoteRelPath::parse(p).unwrap();
        let daily = rel("daily/a.md");
        let (b, c) = (rel("stories/b.md"), rel("stories/c.md"));
        let mut state = NotesListState {
            selected_path: Some(c.clone()),
            notes: [
                (NoteDir::Daily, vec![daily.clone()]),
                (NoteDir::Stories, vec![b, c.clone()]),
            ]
            .into(),
            ..Default::default()
        };

        update(&mut state, NotesListMsg::FocusSectionNext);
        assert_eq!(state.selected_path, Some(daily));
        update(&mut state, NotesListMsg::FocusSectionPrev);
        assert_eq!(state.selected_path, Some(c));
    }
}
