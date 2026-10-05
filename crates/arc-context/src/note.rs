use std::path::Path;

use anyhow::{Context, bail};
use arc_index::Connection;
use arc_notes::note::frontmatter::{EntityLink, Frontmatter, add_branch};
use arc_notes::{BranchName, NoteDir, NoteRelPath};
use arc_shortcut::Cache;
use arc_shortcut::api::story::{Story, iteration_of};
use slugify::slugify;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NoteLinks {
    pub iteration_url: Option<String>,
    pub epic_url: Option<String>,
}

impl NoteLinks {
    pub fn for_story(cache: &Cache, story: &Story) -> Self {
        let iteration_url = cache
            .iteration_for(story)
            .or_else(|| iteration_of(story, &cache.iterations))
            .map(|it| it.app_url.clone());
        let epic_url = story.epic_id.and_then(|id| {
            cache
                .epics
                .iter()
                .find(|e| e.id == id)
                .map(|e| e.app_url.clone())
        });
        Self {
            iteration_url,
            epic_url,
        }
    }
}

pub fn create_story_note(
    conn: &Connection,
    notes_dir: &Path,
    story: &Story,
    links: NoteLinks,
    body: &str,
) -> anyhow::Result<(String, NoteRelPath)> {
    let existing = match arc_index::note_for_story(conn, story.id)? {
        Some(rel) => Some(rel),
        None => arc_index::scan_story_note(notes_dir, story.id)?.map(|(rel, _)| rel),
    };
    if let Some(existing) = existing {
        bail!(
            "story {} already has a note at '{existing}'; use append_note",
            story.id
        );
    }
    let base = slugify!(&story.name);
    if base.is_empty() {
        bail!("story name produced an empty slug");
    }
    let Some((slug, rel)) = [base.clone(), format!("{base}-{}", story.id)]
        .into_iter()
        .map(|slug| {
            let rel = NoteRelPath::in_dir(NoteDir::Stories, &slug);
            (slug, rel)
        })
        .find(|(_, rel)| !rel.abs(notes_dir).exists())
    else {
        bail!(
            "notes '{}' and '{}' already exist",
            NoteRelPath::in_dir(NoteDir::Stories, &base),
            NoteRelPath::in_dir(NoteDir::Stories, &format!("{base}-{}", story.id))
        );
    };
    let abs = rel.abs(notes_dir);
    let fm = Frontmatter::linked(
        slug.clone(),
        EntityLink::Story {
            id: story.id,
            url: story.app_url.clone(),
            name: Some(story.name.clone()),
            iteration_url: links.iteration_url,
            epic_url: links.epic_url,
        },
    );
    let heading = format!("{}\n# {}\n", fm.to_block()?, story.name);
    let content = match body {
        "" => heading,
        body => format!("{heading}\n{body}\n"),
    };
    if let Some(p) = abs.parent() {
        std::fs::create_dir_all(p)?;
    }
    std::fs::write(&abs, &content)?;
    arc_index::reindex_note(conn, &rel, &content)?;
    Ok((slug, rel))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkOutcome {
    Added,
    AlreadyLinked,
}

pub fn link_branch(
    conn: &Connection,
    notes_dir: &Path,
    rel: &NoteRelPath,
    branch: &BranchName,
) -> anyhow::Result<LinkOutcome> {
    let content =
        arc_index::read_note(notes_dir, rel)?.with_context(|| format!("no note at '{rel}'"))?;
    let updated = add_branch(&content, branch)
        .with_context(|| format!("can't add branch to the frontmatter of '{rel}'"))?;
    if updated == content {
        arc_index::reindex_note(conn, rel, &content)?;
        return Ok(LinkOutcome::AlreadyLinked);
    }
    std::fs::write(rel.abs(notes_dir), &updated)?;
    arc_index::reindex_note(conn, rel, &updated)?;
    Ok(LinkOutcome::Added)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rel(s: &str) -> NoteRelPath {
        NoteRelPath::parse(s).unwrap()
    }

    fn story(id: i32, name: &str) -> Story {
        Story {
            id: id.into(),
            name: name.into(),
            app_url: format!("https://app.shortcut.com/x/story/{id}"),
            ..Default::default()
        }
    }

    fn setup() -> (tempfile::TempDir, tempfile::TempDir, Connection) {
        let cache = tempfile::tempdir().unwrap();
        let notes = tempfile::tempdir().unwrap();
        let conn = arc_index::open(cache.path()).unwrap();
        (cache, notes, conn)
    }

    fn no_links() -> NoteLinks {
        NoteLinks::default()
    }

    #[test]
    fn create_story_note_links_and_rejects_duplicates() {
        let (_c, notes, conn) = setup();
        let s = story(42, "Optical Flow");
        let links = NoteLinks {
            iteration_url: Some("https://it".into()),
            epic_url: Some("https://epic".into()),
        };
        let created = create_story_note(&conn, notes.path(), &s, links, "hi").unwrap();
        assert_eq!(
            created,
            ("optical-flow".into(), rel("stories/optical-flow.md"))
        );
        assert_eq!(
            arc_index::note_for_story(&conn, 42.into()).unwrap(),
            Some(rel("stories/optical-flow.md"))
        );
        let body = std::fs::read_to_string(notes.path().join("stories/optical-flow.md")).unwrap();
        let fm = Frontmatter::from_note(&body).unwrap();
        assert!(matches!(
            fm.link,
            EntityLink::Story { id, epic_url: Some(ref e), iteration_url: Some(_), .. } if id.get() == 42 && e == "https://epic"
        ));
        let err = create_story_note(&conn, notes.path(), &s, no_links(), "again").unwrap_err();
        assert!(err.to_string().contains("stories/optical-flow.md"), "{err}");
    }

    #[test]
    fn create_story_note_sidesteps_another_storys_slug() {
        let (_c, notes, conn) = setup();
        create_story_note(&conn, notes.path(), &story(1, "Same Name"), no_links(), "a").unwrap();
        let created =
            create_story_note(&conn, notes.path(), &story(2, "Same Name"), no_links(), "b")
                .unwrap();
        assert_eq!(
            created,
            ("same-name-2".into(), rel("stories/same-name-2.md"))
        );
        assert_eq!(
            arc_index::note_for_story(&conn, 2.into()).unwrap(),
            Some(rel("stories/same-name-2.md"))
        );
        let (rel_1, body) = arc_index::story_note(&conn, notes.path(), 1.into())
            .unwrap()
            .unwrap();
        assert_eq!(rel_1, "stories/same-name.md");
        assert!(body.ends_with("a\n"));

        std::fs::write(
            notes.path().join("stories/other.md"),
            "---\nid: other\n---\n",
        )
        .unwrap();
        let created =
            create_story_note(&conn, notes.path(), &story(3, "Other"), no_links(), "c").unwrap();
        assert_eq!(created.1, "stories/other-3.md");
    }

    #[test]
    fn create_story_note_finds_unindexed_note() {
        let (_c, notes, conn) = setup();
        std::fs::create_dir_all(notes.path().join("stories")).unwrap();
        std::fs::write(
            notes.path().join("stories/renamed.md"),
            "---\nid: renamed\nstory_id: sc-5\n---\n",
        )
        .unwrap();
        let err = create_story_note(&conn, notes.path(), &story(5, "Five"), no_links(), "x")
            .unwrap_err()
            .to_string();
        assert!(err.contains("stories/renamed.md"), "{err}");
        assert!(!notes.path().join("stories/five.md").exists());
    }

    #[test]
    fn note_links_come_from_cached_iterations_and_epics() {
        let mut cache = Cache::default();
        cache.epics = vec![arc_shortcut::api::epic::EpicSlim {
            id: 40.into(),
            name: "Forty".into(),
            app_url: "https://ep/40".into(),
            owner_ids: vec![],
            created_at: "2025-11-01T00:00:00Z".parse().unwrap(),
        }];
        let s = Story {
            epic_id: Some(40.into()),
            ..story(1, "a")
        };
        assert_eq!(
            NoteLinks::for_story(&cache, &s),
            NoteLinks {
                iteration_url: None,
                epic_url: Some("https://ep/40".into()),
            }
        );
    }

    #[test]
    fn link_branch_adds_once_and_reindexes() {
        let (_c, notes, conn) = setup();
        let (_, path) =
            create_story_note(&conn, notes.path(), &story(7, "Seven"), no_links(), "").unwrap();
        let branch = BranchName::parse("me/feature").unwrap();
        assert_eq!(
            link_branch(&conn, notes.path(), &path, &branch).unwrap(),
            LinkOutcome::Added
        );
        assert_eq!(
            link_branch(&conn, notes.path(), &path, &branch).unwrap(),
            LinkOutcome::AlreadyLinked
        );
        assert_eq!(
            arc_index::story_for_branch(&conn, &branch).unwrap(),
            Some((7.into(), path.clone()))
        );
        let body = std::fs::read_to_string(path.abs(notes.path())).unwrap();
        assert_eq!(
            Frontmatter::from_note(&body).unwrap().branches,
            vec!["me/feature"]
        );
        assert!(body.ends_with("---\n\n# Seven\n"));
    }

    #[test]
    fn link_branch_surfaces_unsafe_layouts_and_missing_notes() {
        let (_c, notes, conn) = setup();
        std::fs::create_dir_all(notes.path().join("stories")).unwrap();
        let content = "---\nstory_id: sc-7\nbranches: {a: b}\n---\n";
        std::fs::write(notes.path().join("stories/odd.md"), content).unwrap();
        let branch = BranchName::parse("b").unwrap();
        let err = link_branch(&conn, notes.path(), &rel("stories/odd.md"), &branch).unwrap_err();
        assert!(
            format!("{err:#}").contains("layout not supported"),
            "{err:#}"
        );
        assert_eq!(
            std::fs::read_to_string(notes.path().join("stories/odd.md")).unwrap(),
            content
        );
        assert!(link_branch(&conn, notes.path(), &rel("stories/none.md"), &branch).is_err());
    }
}
