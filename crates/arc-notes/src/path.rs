use std::fmt;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, anyhow, bail};
use serde::{Deserialize, Serialize};

use crate::scope::{PathSegment, RepoKey};

const TODOS_DIR: &str = "todos";

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
        let parts = Path::new(rel)
            .components()
            .filter_map(|c| match c {
                Component::Normal(p) => Some(
                    p.to_str()
                        .ok_or_else(|| anyhow!("path must be valid UTF-8: '{rel}'")),
                ),
                Component::CurDir => None,
                _ => Some(Err(anyhow!(
                    "path must be relative to the notes dir without `..`: '{rel}'"
                ))),
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
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
        is_user_note(rel).then_some(())?;
        Self::from_stored(rel.to_str()?.to_owned())
    }

    pub fn from_stored(rel: String) -> Option<Self> {
        let components = || Path::new(&rel).components();
        let valid = components().next().is_some()
            && components().all(|c| matches!(c, Component::Normal(_)));
        valid.then_some(Self(rel))
    }

    pub fn in_dir(dir: NoteDir, slug: &str) -> Self {
        Self(format!("{dir}/{slug}.md"))
    }

    pub fn todos() -> Self {
        Self("todos.md".into())
    }

    pub fn repo_todos(repo: &RepoKey) -> Self {
        let segments: Vec<&str> = repo.segments().map(PathSegment::as_str).collect();
        Self(format!("{TODOS_DIR}/{}.md", segments.join("/")))
    }

    pub fn is_todo_list(&self) -> bool {
        *self == Self::todos() || self.0.starts_with(&format!("{TODOS_DIR}/"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn abs(&self, notes_dir: &Path) -> PathBuf {
        notes_dir.join(&self.0)
    }

    pub fn abs_within(&self, notes_dir: &Path) -> anyhow::Result<PathBuf> {
        let abs = self.abs(notes_dir);
        let root = match notes_dir.canonicalize() {
            Ok(root) => root,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(abs),
            Err(e) => return Err(e).with_context(|| format!("resolve {}", notes_dir.display())),
        };
        let nearest = abs
            .ancestors()
            .find(|p| p.symlink_metadata().is_ok())
            .unwrap_or(notes_dir);
        let resolved = nearest
            .canonicalize()
            .with_context(|| format!("resolve {}", nearest.display()))?;
        if !resolved.starts_with(&root) {
            bail!("note '{self}' resolves outside the notes dir");
        }
        Ok(abs)
    }

    pub fn file_stem(&self) -> Option<&str> {
        Path::new(&self.0).file_stem().and_then(|s| s.to_str())
    }
}

const SYNC_CONFLICT_MARKER: &str = ".sync-conflict-";

fn is_user_note(rel: &Path) -> bool {
    let markdown = rel.extension().is_some_and(|e| e.eq_ignore_ascii_case("md"));
    let visible = rel.components().all(|c| !c.as_os_str().as_encoded_bytes().starts_with(b"."));
    let conflict_copy = rel
        .file_stem()
        .and_then(|stem| stem.to_str())
        .is_some_and(|stem| stem.contains(SYNC_CONFLICT_MARKER));
    markdown && visible && !conflict_copy
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

    #[cfg(unix)]
    #[test]
    fn abs_within_refuses_symlinks_out_of_the_notes_dir() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let (notes, outside) = (tmp.path().join("notes"), tmp.path().join("outside"));
        std::fs::create_dir_all(notes.join("stories")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.md"), "x").unwrap();
        std::fs::write(notes.join("real.md"), "x").unwrap();
        symlink(outside.join("secret.md"), notes.join("leak.md")).unwrap();
        symlink(&outside, notes.join("escape")).unwrap();
        symlink(notes.join("real.md"), notes.join("alias.md")).unwrap();
        symlink(tmp.path().join("missing.md"), notes.join("dangling.md")).unwrap();
        let within = |rel: &str| NoteRelPath::parse(rel).unwrap().abs_within(&notes);

        assert_eq!(within("real.md").unwrap(), notes.join("real.md"));
        assert_eq!(within("alias.md").unwrap(), notes.join("alias.md"));
        assert_eq!(
            within("stories/new.md").unwrap(),
            notes.join("stories/new.md")
        );
        assert_eq!(
            within("new/dir/note.md").unwrap(),
            notes.join("new/dir/note.md")
        );
        for escaping in [
            "leak.md",
            "escape/secret.md",
            "escape/new.md",
            "dangling.md",
        ] {
            assert!(within(escaping).is_err(), "{escaping}");
        }
        let missing = tmp.path().join("no-notes");
        assert_eq!(
            NoteRelPath::parse("a.md")
                .unwrap()
                .abs_within(&missing)
                .unwrap(),
            missing.join("a.md")
        );
    }

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
    fn nested_repo_todo_notes_are_todo_lists() {
        let nested = NoteRelPath::repo_todos(&RepoKey::canonical("gitlab.com/g/sub/r").unwrap());
        assert_eq!(nested.as_str(), "todos/gitlab.com/g/sub/r.md");
        assert!(nested.is_todo_list());
        assert!(NoteRelPath::todos().is_todo_list());
        assert!(!NoteRelPath::parse("stories/todos.md").unwrap().is_todo_list());
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
            NoteRelPath::from_abs(notes, Path::new("/home/u/notes/a.MD")).unwrap().as_str(),
            "a.MD"
        );
        assert!(NoteRelPath::from_abs(notes, Path::new("/home/u/notes/a.txt")).is_none());
    }

    #[test]
    fn from_abs_rejects_sync_and_tool_artefacts() {
        let notes = Path::new("/home/u/notes");
        for artefact in [
            ".stversions/todos~20261009-000000.md",
            ".stfolder/x.md",
            ".git/x.md",
            ".obsidian/workspace.md",
            ".trash/old.md",
            "stories/.hidden.md",
            "todos.sync-conflict-20261009-000000-ABCDEFG.md",
            "todos/github.com/o/arc.sync-conflict-20261009-000000-ABCDEFG.md",
        ] {
            assert!(NoteRelPath::from_abs(notes, &notes.join(artefact)).is_none(), "{artefact}");
        }
        assert!(NoteRelPath::from_abs(notes, &notes.join("todos/github.com/o/my.repo.md")).is_some());
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
