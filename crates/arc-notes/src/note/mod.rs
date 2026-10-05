use std::path::{Path, PathBuf};

use slugify::slugify;

use crate::note::frontmatter::Frontmatter;

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
        let frontmatter =
            Frontmatter::story(slug.clone(), story_id, story_name, story_app_url, iteration_app_url);
        let mut path = PathBuf::from(notes_dir.as_ref());
        path.push("stories");
        path.push(format!("{slug}.md"));

        Self { frontmatter, path }
    }
}
