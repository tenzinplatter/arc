use std::fmt;
use std::path::{Component, Path, PathBuf};

use anyhow::{anyhow, bail};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NoteDir {
    Daily,
    Stories,
    Iterations,
    Epics,
    Scratch,
}

impl NoteDir {
    pub const ALL: [NoteDir; 5] = [
        NoteDir::Daily,
        NoteDir::Stories,
        NoteDir::Iterations,
        NoteDir::Epics,
        NoteDir::Scratch,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            NoteDir::Daily => "daily",
            NoteDir::Stories => "stories",
            NoteDir::Iterations => "iterations",
            NoteDir::Epics => "epics",
            NoteDir::Scratch => "scratch",
        }
    }
}

impl fmt::Display for NoteDir {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl AsRef<Path> for NoteDir {
    fn as_ref(&self) -> &Path {
        Path::new(self.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NoteRelPath(String);

impl NoteRelPath {
    pub fn parse(rel: &str) -> anyhow::Result<Self> {
        let mut parts = Vec::new();
        for c in Path::new(rel).components() {
            match c {
                Component::Normal(p) => parts.push(
                    p.to_str()
                        .ok_or_else(|| anyhow!("path must be valid UTF-8: '{rel}'"))?,
                ),
                Component::CurDir => {}
                _ => bail!("path must be relative to the notes dir without `..`: '{rel}'"),
            }
        }
        if parts.is_empty() {
            bail!("path must be relative to the notes dir without `..`: '{rel}'");
        }
        let norm = parts.join("/");
        if Path::new(&norm).extension().is_none_or(|e| e != "md") {
            bail!("path must be a .md note: '{rel}'");
        }
        Ok(Self(norm))
    }

    pub fn from_abs(notes_dir: &Path, abs: &Path) -> Option<Self> {
        let rel = abs.strip_prefix(notes_dir).ok()?;
        Self::from_stored(rel.to_str()?.to_owned())
    }

    pub fn from_stored(rel: String) -> Option<Self> {
        let mut components = Path::new(&rel).components().peekable();
        components.peek()?;
        components
            .all(|c| matches!(c, Component::Normal(_)))
            .then_some(Self(rel))
    }

    pub fn in_dir(dir: NoteDir, slug: &str) -> Self {
        Self(format!("{dir}/{slug}.md"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn abs(&self, notes_dir: &Path) -> PathBuf {
        notes_dir.join(&self.0)
    }

    pub fn file_stem(&self) -> Option<&str> {
        Path::new(&self.0).file_stem().and_then(|s| s.to_str())
    }
}

impl fmt::Display for NoteRelPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl PartialEq<str> for NoteRelPath {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl PartialEq<&str> for NoteRelPath {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

impl AsRef<Path> for NoteRelPath {
    fn as_ref(&self) -> &Path {
        Path::new(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_normalises_and_rejects_escapes() {
        assert_eq!(NoteRelPath::parse("stories/a.md").unwrap().as_str(), "stories/a.md");
        assert_eq!(NoteRelPath::parse("./a.md").unwrap().as_str(), "a.md");
        assert_eq!(
            NoteRelPath::parse(".//stories//./a.md").unwrap().as_str(),
            "stories/a.md"
        );
        for bad in ["../a.md", "stories/../../a.md", "/etc/a.md", "", ".", "a.txt", "stories"] {
            assert!(NoteRelPath::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn note_dirs_name_their_on_disk_folders() {
        let names: Vec<&str> = NoteDir::ALL.iter().map(|d| d.as_str()).collect();
        assert_eq!(names, ["daily", "stories", "iterations", "epics", "scratch"]);
        assert_eq!(NoteRelPath::in_dir(NoteDir::Stories, "a-b").as_str(), "stories/a-b.md");
    }

    #[test]
    fn from_abs_is_relative_to_notes_dir() {
        let notes = Path::new("/home/u/notes");
        assert_eq!(
            NoteRelPath::from_abs(notes, Path::new("/home/u/notes/stories/a.md"))
                .unwrap()
                .as_str(),
            "stories/a.md"
        );
        assert!(NoteRelPath::from_abs(notes, Path::new("/elsewhere/a.md")).is_none());
        assert!(NoteRelPath::from_abs(notes, Path::new("/home/u/notes/../x/a.md")).is_none());
        assert!(NoteRelPath::from_abs(notes, notes).is_none());
        assert_eq!(
            NoteRelPath::from_abs(notes, Path::new("/home/u/notes/a.TXT")).unwrap().as_str(),
            "a.TXT"
        );
    }

    #[test]
    fn from_stored_rejects_empty_absolute_and_escaping_paths() {
        assert_eq!(NoteRelPath::from_stored("stories/a.md".into()).unwrap().as_str(), "stories/a.md");
        assert_eq!(NoteRelPath::from_stored("notes.txt".into()).unwrap().as_str(), "notes.txt");
        for bad in ["", "/abs/a.md", "../a.md", "stories/../a.md", "./a.md"] {
            assert!(NoteRelPath::from_stored(bad.into()).is_none(), "{bad}");
        }
    }
}
