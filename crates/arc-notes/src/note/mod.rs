use std::path::{Path, PathBuf};

use slugify::slugify;

use crate::note::frontmatter::Frontmatter;
use crate::path::{NoteDir, NoteRelPath};

pub mod frontmatter;

pub struct Note {
    pub frontmatter: Frontmatter,
    pub path: PathBuf,
}

impl Note {
    pub fn new<P: AsRef<Path>>(
        notes_dir: &P,
        story_id: arc_core::StoryId,
        story_name: String,
        story_app_url: String,
        iteration_app_url: Option<String>,
    ) -> Self {
        let slug = slugify!(&story_name);
        let path = NoteRelPath::in_dir(NoteDir::Stories, &slug).abs(notes_dir.as_ref());
        let frontmatter =
            Frontmatter::story(slug, story_id, story_name, story_app_url, iteration_app_url);
        Self { frontmatter, path }
    }
}
