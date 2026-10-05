use std::path::Path;

use anyhow::{Context, bail};
use arc_core::StoryId;
use arc_index::Connection;
use arc_notes::note::frontmatter::{EntityLink, Frontmatter, add_branch, replace_branch};
use arc_notes::{BranchName, NoteDir, NoteRelPath};
use arc_shortcut::Cache;
use arc_shortcut::api::story::{Story, iteration_of};
use slugify::slugify;

use crate::branch::LinkableBranch;

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoryNote {
    Created { slug: String, rel: NoteRelPath },
    Existing(NoteRelPath),
}

fn live_indexed_note(
    conn: &Connection,
    notes_dir: &Path,
    id: StoryId,
) -> anyhow::Result<Option<NoteRelPath>> {
    while let Some(rel) = arc_index::note_for_story(conn, id)? {
        if rel.abs(notes_dir).exists() {
            return Ok(Some(rel));
        }
        arc_index::remove_note(conn, &rel)?;
    }
    Ok(None)
}

pub fn create_story_note(
    conn: &Connection,
    notes_dir: &Path,
    story: &Story,
    links: NoteLinks,
    body: &str,
) -> anyhow::Result<StoryNote> {
    let existing = match live_indexed_note(conn, notes_dir, story.id)? {
        Some(rel) => Some(rel),
        None => arc_index::scan_story_note(notes_dir, story.id)?.map(|(rel, _)| rel),
    };
    if let Some(existing) = existing {
        return Ok(StoryNote::Existing(existing));
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
    Ok(StoryNote::Created { slug, rel })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkOutcome {
    Added,
    AlreadyLinked,
}

fn edit_note(
    conn: &Connection,
    notes_dir: &Path,
    rel: &NoteRelPath,
    edit: impl FnOnce(&str) -> anyhow::Result<String>,
) -> anyhow::Result<bool> {
    let content =
        arc_index::read_note(notes_dir, rel)?.with_context(|| format!("no note at '{rel}'"))?;
    let updated = edit(&content)?;
    if updated == content {
        arc_index::reindex_note(conn, rel, &content)?;
        return Ok(false);
    }
    std::fs::write(rel.abs(notes_dir), &updated)?;
    arc_index::reindex_note(conn, rel, &updated)?;
    Ok(true)
}

pub fn link_branch(
    conn: &Connection,
    notes_dir: &Path,
    rel: &NoteRelPath,
    branch: &LinkableBranch,
) -> anyhow::Result<LinkOutcome> {
    let added = edit_note(conn, notes_dir, rel, |content| {
        add_branch(content, branch.as_branch())
            .with_context(|| format!("can't add branch to the frontmatter of '{rel}'"))
    })?;
    Ok(match added {
        true => LinkOutcome::Added,
        false => LinkOutcome::AlreadyLinked,
    })
}

#[derive(Debug)]
pub enum NoteRename {
    NotListed,
    Updated(NoteRelPath),
    Failed(NoteRelPath, anyhow::Error),
}

pub fn rename_linked_branch(
    conn: &Connection,
    notes_dir: &Path,
    rel: &NoteRelPath,
    from: &BranchName,
    to: &BranchName,
) -> NoteRename {
    let renamed = edit_note(conn, notes_dir, rel, |content| {
        replace_branch(content, from, to)
            .with_context(|| format!("can't rename the branch in the frontmatter of '{rel}'"))
    });
    match renamed {
        Ok(true) => NoteRename::Updated(rel.clone()),
        Ok(false) => NoteRename::NotListed,
        Err(e) => NoteRename::Failed(rel.clone(), e),
    }
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

    fn br(name: &str) -> BranchName {
        BranchName::parse(name).unwrap()
    }

    fn linkable(name: &str) -> LinkableBranch {
        LinkableBranch::parse(br(name), None).unwrap()
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
            StoryNote::Created {
                slug: "optical-flow".into(),
                rel: rel("stories/optical-flow.md")
            }
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
        assert_eq!(
            create_story_note(&conn, notes.path(), &s, no_links(), "again").unwrap(),
            StoryNote::Existing(rel("stories/optical-flow.md"))
        );
        assert!(
            std::fs::read_to_string(notes.path().join("stories/optical-flow.md"))
                .unwrap()
                .ends_with("hi\n")
        );
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
            StoryNote::Created {
                slug: "same-name-2".into(),
                rel: rel("stories/same-name-2.md")
            }
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
        assert!(
            matches!(created, StoryNote::Created { ref rel, .. } if rel == "stories/other-3.md"),
            "{created:?}"
        );
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
        assert_eq!(
            create_story_note(&conn, notes.path(), &story(5, "Five"), no_links(), "x").unwrap(),
            StoryNote::Existing(rel("stories/renamed.md"))
        );
        assert!(!notes.path().join("stories/five.md").exists());
    }

    #[test]
    fn create_story_note_replaces_stale_index_rows() {
        let (_c, notes, conn) = setup();
        arc_index::reindex_note(&conn, &rel("stories/gone.md"), "---\nstory_id: sc-5\n---\n")
            .unwrap();
        arc_index::reindex_note(&conn, &rel("work/gone.md"), "---\nstory_id: sc-5\n---\n").unwrap();
        assert_eq!(
            create_story_note(&conn, notes.path(), &story(5, "Five"), no_links(), "").unwrap(),
            StoryNote::Created {
                slug: "five".into(),
                rel: rel("stories/five.md")
            }
        );
        assert_eq!(
            arc_index::note_for_story(&conn, 5.into()).unwrap(),
            Some(rel("stories/five.md"))
        );
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM notes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn create_story_note_with_empty_body_ends_at_the_heading() {
        let (_c, notes, conn) = setup();
        create_story_note(&conn, notes.path(), &story(6, "Six"), no_links(), "").unwrap();
        let body = std::fs::read_to_string(notes.path().join("stories/six.md")).unwrap();
        assert!(body.ends_with("---\n\n# Six\n"), "{body:?}");
        create_story_note(&conn, notes.path(), &story(9, "Nine"), no_links(), "text").unwrap();
        let body = std::fs::read_to_string(notes.path().join("stories/nine.md")).unwrap();
        assert!(body.ends_with("---\n\n# Nine\n\ntext\n"), "{body:?}");
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
        let StoryNote::Created { rel: path, .. } =
            create_story_note(&conn, notes.path(), &story(7, "Seven"), no_links(), "").unwrap()
        else {
            panic!("expected a new note");
        };
        let branch = linkable("me/feature");
        assert_eq!(
            link_branch(&conn, notes.path(), &path, &branch).unwrap(),
            LinkOutcome::Added
        );
        assert_eq!(
            link_branch(&conn, notes.path(), &path, &branch).unwrap(),
            LinkOutcome::AlreadyLinked
        );
        assert_eq!(
            arc_index::story_for_branch(&conn, branch.as_branch()).unwrap(),
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
        let branch = linkable("b");
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

    #[test]
    fn rename_linked_branch_reports_each_outcome() {
        let (_c, notes, conn) = setup();
        std::fs::create_dir_all(notes.path().join("stories")).unwrap();
        let write = |name: &str, content: &str| {
            std::fs::write(notes.path().join("stories").join(name), content).unwrap();
        };
        write(
            "a.md",
            "---\nstory_id: sc-7\nbranches: [wip, other]\n---\nwip\n",
        );
        let renamed = rename_linked_branch(
            &conn,
            notes.path(),
            &rel("stories/a.md"),
            &br("wip"),
            &br("me/sc-7/seven"),
        );
        assert!(
            matches!(renamed, NoteRename::Updated(ref r) if r == "stories/a.md"),
            "{renamed:?}"
        );
        assert_eq!(
            std::fs::read_to_string(notes.path().join("stories/a.md")).unwrap(),
            "---\nstory_id: sc-7\nbranches: [me/sc-7/seven, other]\n---\nwip\n"
        );
        assert_eq!(
            arc_index::story_for_branch(&conn, &br("me/sc-7/seven")).unwrap(),
            Some((7.into(), rel("stories/a.md")))
        );
        assert_eq!(
            arc_index::story_for_branch(&conn, &br("wip")).unwrap(),
            None
        );

        let renamed = rename_linked_branch(
            &conn,
            notes.path(),
            &rel("stories/a.md"),
            &br("wip"),
            &br("x"),
        );
        assert!(matches!(renamed, NoteRename::NotListed), "{renamed:?}");

        let odd = "---\nstory_id: sc-7\nbranches:\n  - a\n# c\n  - wip\n---\n";
        write("odd.md", odd);
        let renamed = rename_linked_branch(
            &conn,
            notes.path(),
            &rel("stories/odd.md"),
            &br("wip"),
            &br("x"),
        );
        assert!(
            matches!(renamed, NoteRename::Failed(ref r, _) if r == "stories/odd.md"),
            "{renamed:?}"
        );
        assert_eq!(
            std::fs::read_to_string(notes.path().join("stories/odd.md")).unwrap(),
            odd
        );
    }
}
