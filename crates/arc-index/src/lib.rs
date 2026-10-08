use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
pub use rusqlite::Connection;
use rusqlite::OpenFlags;

use arc_notes::note::frontmatter::{EntityLink, Frontmatter};
use arc_notes::todos::Todo;
use arc_notes::{BranchName, NoteDir, NoteRelPath};
use arc_core::StoryId;

const DB_FILE: &str = "arc.db";

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS notes (
    path          TEXT PRIMARY KEY,
    id            TEXT,
    title         TEXT,
    created       TEXT,
    type          TEXT,
    link_kind     TEXT,
    entity_id     INTEGER,
    entity_url    TEXT,
    entity_name   TEXT,
    iteration_url TEXT,
    epic_url      TEXT
);
CREATE TABLE IF NOT EXISTS note_tags (
    path TEXT NOT NULL,
    tag  TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS note_branches (
    path   TEXT NOT NULL,
    branch TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS todos (
    id        TEXT PRIMARY KEY,
    text      TEXT,
    date      TEXT,
    completed INTEGER,
    file      TEXT,
    line      INTEGER,
    position  INTEGER
);
CREATE INDEX IF NOT EXISTS idx_note_tags_path ON note_tags(path);
CREATE INDEX IF NOT EXISTS idx_note_branches_branch ON note_branches(branch);
CREATE INDEX IF NOT EXISTS idx_note_branches_path ON note_branches(path);
CREATE INDEX IF NOT EXISTS idx_notes_entity ON notes(link_kind, entity_id);
";

const WRITER_CONTENTION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

fn db_path(cache_dir: &Path) -> PathBuf {
    cache_dir.join(DB_FILE)
}

pub fn open(cache_dir: &Path) -> Result<Connection> {
    open_waiting(cache_dir, WRITER_CONTENTION_TIMEOUT)
}

pub fn open_waiting(cache_dir: &Path, busy: std::time::Duration) -> Result<Connection> {
    std::fs::create_dir_all(cache_dir).ok();
    let conn = Connection::open(db_path(cache_dir))
        .with_context(|| format!("open index at {}", db_path(cache_dir).display()))?;
    conn.busy_timeout(busy)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

pub fn open_readonly(cache_dir: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(db_path(cache_dir), OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("open index (ro) at {}", db_path(cache_dir).display()))?;
    Ok(conn)
}

pub fn open_for_reading(cache_dir: &Path) -> Result<Connection> {
    if db_path(cache_dir).exists() {
        return open_readonly(cache_dir);
    }
    let conn = Connection::open_in_memory()?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

pub fn reindex_note(conn: &Connection, rel: &NoteRelPath, content: &str) -> Result<()> {
    conn.execute_batch("SAVEPOINT reindex_note")?;
    match write_note_rows(conn, rel, content) {
        Ok(()) => {
            conn.execute_batch("RELEASE reindex_note")?;
            Ok(())
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK TO reindex_note; RELEASE reindex_note");
            Err(e)
        }
    }
}

fn write_note_rows(conn: &Connection, rel: &NoteRelPath, content: &str) -> Result<()> {
    let path = rel.as_str().to_string();
    let fm = Frontmatter::from_note(content);
    let title = note_title(content, fm.as_ref());

    let id = fm.as_ref().map(|f| f.id.as_str());
    let created = fm.as_ref().and_then(|f| f.created).map(|d| d.to_string());
    let note_type = fm.as_ref().and_then(|f| f.note_type.as_deref());
    let link = LinkColumns::from(fm.as_ref().map_or(&EntityLink::None, |f| &f.link));

    conn.execute(
        "INSERT OR REPLACE INTO notes
         (path, id, title, created, type, link_kind, entity_id, entity_url, entity_name, iteration_url, epic_url)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        rusqlite::params![
            path,
            id,
            title,
            created,
            note_type,
            link.kind,
            link.entity_id,
            link.url,
            link.name,
            link.iteration_url,
            link.epic_url
        ],
    )?;
    conn.execute("DELETE FROM note_tags WHERE path = ?1", [&path])?;
    conn.execute("DELETE FROM note_branches WHERE path = ?1", [&path])?;
    if let Some(f) = &fm {
        for tag in &f.tags {
            conn.execute(
                "INSERT INTO note_tags (path, tag) VALUES (?1, ?2)",
                rusqlite::params![path, tag],
            )?;
        }
        for branch in &f.branches {
            conn.execute(
                "INSERT INTO note_branches (path, branch) VALUES (?1, ?2)",
                rusqlite::params![path, branch],
            )?;
        }
    }
    Ok(())
}

#[derive(Default)]
struct LinkColumns<'a> {
    kind: Option<&'static str>,
    entity_id: Option<i32>,
    url: Option<&'a str>,
    name: Option<&'a str>,
    iteration_url: Option<&'a str>,
    epic_url: Option<&'a str>,
}

impl<'a> LinkColumns<'a> {
    fn entity(kind: &'static str, id: i32, url: &'a str, name: Option<&'a str>) -> Self {
        Self {
            kind: Some(kind),
            entity_id: Some(id),
            url: Some(url),
            name,
            ..Self::default()
        }
    }
}

impl<'a> From<&'a EntityLink> for LinkColumns<'a> {
    fn from(link: &'a EntityLink) -> Self {
        match link {
            EntityLink::None => Self::default(),
            EntityLink::Story {
                id,
                url,
                name,
                iteration_url,
                epic_url,
            } => Self {
                iteration_url: iteration_url.as_deref(),
                epic_url: epic_url.as_deref(),
                ..Self::entity("story", id.get(), url, name.as_deref())
            },
            EntityLink::Iteration { id, url, name } => {
                Self::entity("iteration", id.get(), url, name.as_deref())
            }
            EntityLink::Epic { id, url, name } => {
                Self::entity("epic", id.get(), url, name.as_deref())
            }
        }
    }
}

pub fn remove_note(conn: &Connection, rel: &NoteRelPath) -> Result<()> {
    let path = rel.as_str().to_string();
    conn.execute("DELETE FROM notes WHERE path = ?1", [&path])?;
    conn.execute("DELETE FROM note_tags WHERE path = ?1", [&path])?;
    conn.execute("DELETE FROM note_branches WHERE path = ?1", [&path])?;
    Ok(())
}

pub fn mirror_todos(conn: &mut Connection, todos: &[Todo]) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM todos", [])?;
    for (pos, t) in todos.iter().enumerate() {
        tx.execute(
            "INSERT INTO todos (id, text, date, completed, file, line, position)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                t.id.to_string(),
                t.text,
                t.date.map(|d| d.to_string()),
                t.completed as i64,
                t.file.as_str(),
                t.line,
                pos as i64,
            ],
        )?;
    }
    tx.commit()?;
    Ok(())
}

#[derive(Debug, Clone)]
pub struct ReadOnlySql(String);

impl ReadOnlySql {
    pub fn parse(sql: &str) -> Result<Self> {
        let head = sql.split_whitespace().next().unwrap_or("");
        if !matches!(
            head.to_ascii_uppercase().as_str(),
            "SELECT" | "WITH" | "EXPLAIN" | "PRAGMA"
        ) {
            anyhow::bail!("only read-only queries (SELECT/WITH/EXPLAIN/PRAGMA) are allowed");
        }
        Ok(Self(sql.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub fn query(conn: &Connection, sql: &ReadOnlySql) -> Result<String> {
    Ok(query_capped(conn, sql, usize::MAX)?.0)
}

pub fn query_capped(
    conn: &Connection,
    sql: &ReadOnlySql,
    max_rows: usize,
) -> Result<(String, bool)> {
    query_with_timeout(conn, sql, max_rows, QUERY_TIMEOUT)
}

const QUERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

fn query_with_timeout(
    conn: &Connection,
    sql: &ReadOnlySql,
    max_rows: usize,
    timeout: std::time::Duration,
) -> Result<(String, bool)> {
    let deadline = std::time::Instant::now() + timeout;
    conn.progress_handler(1000, Some(move || std::time::Instant::now() > deadline))?;
    let result = run_query(conn, sql.as_str(), max_rows);
    conn.progress_handler(0, None::<fn() -> bool>)?;
    result.map_err(|e| match e.downcast_ref::<rusqlite::Error>() {
        Some(rusqlite::Error::SqliteFailure(f, _))
            if f.code == rusqlite::ErrorCode::OperationInterrupted =>
        {
            anyhow::anyhow!("query timed out after {}s", timeout.as_secs_f32())
        }
        _ => e,
    })
}

fn run_query(conn: &Connection, sql: &str, max_rows: usize) -> Result<(String, bool)> {
    let mut stmt = conn.prepare(sql)?;
    if !stmt.readonly() {
        anyhow::bail!("only read-only queries are allowed");
    }
    let cols: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
    let mut objects = stmt.query([])?.mapped(|row| row_to_json(row, &cols));
    let out = objects
        .by_ref()
        .take(max_rows)
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let truncated = objects.next().transpose()?.is_some();
    Ok((serde_json::to_string(&out)?, truncated))
}

fn row_to_json(row: &rusqlite::Row, cols: &[String]) -> rusqlite::Result<serde_json::Value> {
    cols.iter()
        .enumerate()
        .map(|(i, name)| Ok((name.clone(), value_to_json(row.get_ref(i)?))))
        .collect::<rusqlite::Result<serde_json::Map<_, _>>>()
        .map(serde_json::Value::Object)
}

fn stored_paths(rows: impl Iterator<Item = rusqlite::Result<String>>) -> Result<Vec<NoteRelPath>> {
    let paths = rows.collect::<rusqlite::Result<Vec<String>>>()?;
    Ok(paths.into_iter().filter_map(NoteRelPath::from_stored).collect())
}

pub fn note_paths_for_id(conn: &Connection, id: &str) -> Result<Vec<NoteRelPath>> {
    let mut stmt = conn.prepare("SELECT path FROM notes WHERE id = ?1 ORDER BY path")?;
    let rows = stmt.query_map([id], |r| r.get(0))?;
    stored_paths(rows)
}

pub fn note_for_story(conn: &Connection, story_id: StoryId) -> Result<Option<NoteRelPath>> {
    let mut stmt = conn.prepare(
        "SELECT path FROM notes WHERE link_kind = 'story' AND entity_id = ?1
         ORDER BY path NOT LIKE 'stories/%', path",
    )?;
    let rows = stmt.query_map([story_id.get()], |r| r.get(0))?;
    Ok(stored_paths(rows)?.into_iter().next())
}

pub fn story_for_branch(
    conn: &Connection,
    branch: &BranchName,
) -> Result<Option<(StoryId, NoteRelPath)>> {
    let mut stmt = conn.prepare(
        "SELECT n.entity_id, n.path FROM note_branches b
         JOIN notes n ON n.path = b.path
         WHERE b.branch = ?1 AND n.link_kind = 'story'
         ORDER BY n.path NOT LIKE 'stories/%', n.path",
    )?;
    let found = stmt
        .query_map([branch.as_str()], |r| {
            Ok((StoryId::from(r.get::<_, i32>(0)?), r.get::<_, String>(1)?))
        })?
        .find_map(|row| match row {
            Ok((id, path)) => NoteRelPath::from_stored(path).map(|rel| Ok((id, rel))),
            Err(e) => Some(Err(e)),
        })
        .transpose()?;
    Ok(found)
}

pub fn read_note(notes_dir: &Path, rel: &NoteRelPath) -> Result<Option<String>> {
    match std::fs::read_to_string(rel.abs(notes_dir)) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("read note {rel}")),
    }
}

pub fn story_note(
    conn: &Connection,
    notes_dir: &Path,
    story_id: StoryId,
) -> Result<Option<(NoteRelPath, String)>> {
    if let Some(rel) = note_for_story(conn, story_id)?
        && let Some(body) = read_note(notes_dir, &rel)?
    {
        return Ok(Some((rel, body)));
    }
    scan_story_note(notes_dir, story_id)
}

pub fn scan_story_note(
    notes_dir: &Path,
    story_id: StoryId,
) -> Result<Option<(NoteRelPath, String)>> {
    let mut names: Vec<String> = match std::fs::read_dir(notes_dir.join(NoteDir::Stories)) {
        Ok(dir) => dir
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    names.sort();
    for stem in names.iter().filter_map(|n| n.strip_suffix(".md")) {
        let rel = NoteRelPath::in_dir(NoteDir::Stories, stem);
        if let Some(body) = read_note(notes_dir, &rel)?
            && links_story(&body, story_id)
        {
            return Ok(Some((rel, body)));
        }
    }
    Ok(None)
}

fn links_story(content: &str, story_id: StoryId) -> bool {
    Frontmatter::from_note(content)
        .is_some_and(|fm| matches!(fm.link, EntityLink::Story { id, .. } if id == story_id))
}

fn value_to_json(v: rusqlite::types::ValueRef) -> serde_json::Value {
    use rusqlite::types::ValueRef;
    match v {
        ValueRef::Null => serde_json::Value::Null,
        ValueRef::Integer(i) => serde_json::Value::from(i),
        ValueRef::Real(f) => serde_json::Value::from(f),
        ValueRef::Text(t) => serde_json::Value::from(String::from_utf8_lossy(t).into_owned()),
        ValueRef::Blob(b) => serde_json::Value::from(format!("<{} bytes>", b.len())),
    }
}

fn note_title(content: &str, fm: Option<&Frontmatter>) -> Option<String> {
    for line in content.lines() {
        if let Some(h) = line.strip_prefix("# ") {
            return Some(h.trim().to_string());
        }
    }
    fm.map(|f| f.id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    use chrono::NaiveDate;

    fn rel(s: &str) -> NoteRelPath {
        NoteRelPath::parse(s).unwrap()
    }

    fn branch(s: &str) -> BranchName {
        BranchName::parse(s).unwrap()
    }

    fn select(conn: &Connection, sql: &str) -> Result<String> {
        query(conn, &ReadOnlySql::parse(sql)?)
    }

    fn select_capped(conn: &Connection, sql: &str, max_rows: usize) -> Result<(String, bool)> {
        query_capped(conn, &ReadOnlySql::parse(sql)?, max_rows)
    }

    #[test]
    fn indexes_and_queries_a_story_note() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = open(tmp.path()).unwrap();
        let content = "---\nid: my-story\ntags: [backend]\ncreated: \"2026-03-31\"\nstory_id: sc-19559\nstory_link: https://s/19559\n---\n\n# Ship the thing\n\nbody";
        reindex_note(&conn, &rel("stories/my-story.md"), content).unwrap();

        let rows = select(
            &conn,
            "SELECT id, title, entity_id, link_kind FROM notes WHERE link_kind = 'story'",
        )
        .unwrap();
        assert!(rows.contains("my-story"));
        assert!(rows.contains("Ship the thing"));
        assert!(rows.contains("19559"));

        let tags = select(&conn, "SELECT tag FROM note_tags").unwrap();
        assert!(tags.contains("backend"));
    }

    #[test]
    fn reindex_replaces_not_duplicates() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = open(tmp.path()).unwrap();
        let p = &rel("a.md");
        reindex_note(&conn, p, "---\nid: a\n---\n").unwrap();
        reindex_note(&conn, p, "---\nid: a\n---\n").unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM notes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn mirrors_and_gets_note_and_rejects_writes() {
        let tmp = tempfile::tempdir().unwrap();
        let notes = tmp.path().join("notes");
        std::fs::create_dir_all(notes.join("stories")).unwrap();
        std::fs::write(notes.join("stories/a.md"), "---\nid: a\n---\n# Hi\n").unwrap();

        let mut conn = open(tmp.path()).unwrap();
        reindex_note(&conn, &rel("stories/a.md"), "---\nid: a\n---\n# Hi\n").unwrap();

        let todos = arc_notes::scanner::parse_note("- [ ] do it @01-05-2026", &rel("todos.md"));
        mirror_todos(&mut conn, &todos).unwrap();
        let got = select(&conn, "SELECT text FROM todos").unwrap();
        assert!(got.contains("do it"));

        let body = read_note(&notes, &rel("stories/a.md")).unwrap().unwrap();
        assert!(body.contains("# Hi"));

        assert!(select(&conn, "DELETE FROM todos").is_err());
    }

    #[test]
    fn indexes_branches_and_clears_on_remove() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = open(tmp.path()).unwrap();
        let p = &rel("stories/s.md");
        let content = "---\nid: s\nstory_id: sc-42\nbranches: [me/sc-42/s, stacked]\n---\n";
        reindex_note(&conn, p, content).unwrap();
        assert_eq!(
            story_for_branch(&conn, &branch("stacked")).unwrap(),
            Some((42.into(), rel("stories/s.md")))
        );
        assert_eq!(story_for_branch(&conn, &branch("nope")).unwrap(), None);

        reindex_note(
            &conn,
            p,
            "---\nid: s\nstory_id: sc-42\nbranches: [stacked]\n---\n",
        )
        .unwrap();
        assert_eq!(story_for_branch(&conn, &branch("me/sc-42/s")).unwrap(), None);

        remove_note(&conn, p).unwrap();
        assert_eq!(story_for_branch(&conn, &branch("stacked")).unwrap(), None);
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM note_branches", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn stored_rows_with_unsafe_paths_are_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = open(tmp.path()).unwrap();
        let content = "---\nid: s\nstory_id: sc-7\nbranches: [b]\n---\n";
        reindex_note(&conn, &rel("zz.md"), content).unwrap();
        for legacy in ["/abs/s.md", "../s.md"] {
            conn.execute(
                "INSERT INTO notes (path, id, link_kind, entity_id) VALUES (?1, 's', 'story', 7)",
                [legacy],
            )
            .unwrap();
            conn.execute("INSERT INTO note_branches (path, branch) VALUES (?1, 'b')", [legacy])
                .unwrap();
        }
        assert_eq!(note_paths_for_id(&conn, "s").unwrap(), vec![rel("zz.md")]);
        assert_eq!(note_for_story(&conn, 7.into()).unwrap(), Some(rel("zz.md")));
        assert_eq!(
            story_for_branch(&conn, &branch("b")).unwrap(),
            Some((7.into(), rel("zz.md")))
        );
    }

    #[test]
    fn branches_on_non_story_notes_dont_resolve() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = open(tmp.path()).unwrap();
        reindex_note(&conn, &rel("x.md"), "---\nid: x\nbranches: [b]\n---\n").unwrap();
        assert_eq!(story_for_branch(&conn, &branch("b")).unwrap(), None);
    }

    #[test]
    fn note_for_story_prefers_stories_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = open(tmp.path()).unwrap();
        let fm = "---\nid: s\nstory_id: sc-7\n---\n";
        reindex_note(&conn, &rel("archive/s.md"), fm).unwrap();
        reindex_note(&conn, &rel("stories/s.md"), fm).unwrap();
        assert_eq!(
            note_for_story(&conn, 7.into()).unwrap(),
            Some(rel("stories/s.md"))
        );
        assert_eq!(note_for_story(&conn, 8.into()).unwrap(), None);
    }

    #[test]
    fn note_paths_for_colliding_ids() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = open(tmp.path()).unwrap();
        reindex_note(&conn, &rel("stories/foo.md"), "---\nid: foo\n---\n").unwrap();
        reindex_note(&conn, &rel("foo.md"), "---\nid: foo\n---\n").unwrap();
        assert_eq!(
            note_paths_for_id(&conn, "foo").unwrap(),
            vec![rel("foo.md"), rel("stories/foo.md")]
        );
        assert!(note_paths_for_id(&conn, "bar").unwrap().is_empty());
    }

    #[test]
    fn open_for_reading_never_creates_or_writes_the_index() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = open_for_reading(tmp.path()).unwrap();
        assert_eq!(story_for_branch(&conn, &branch("b")).unwrap(), None);
        assert!(!db_path(tmp.path()).exists());

        let writer = open(tmp.path()).unwrap();
        reindex_note(&writer, &rel("stories/a.md"), "---\nstory_id: sc-4\nbranches: [b]\n---\n")
            .unwrap();
        let conn = open_for_reading(tmp.path()).unwrap();
        assert_eq!(
            story_for_branch(&conn, &branch("b")).unwrap(),
            Some((4.into(), rel("stories/a.md")))
        );
        assert!(reindex_note(&conn, &rel("x.md"), "x").is_err());
    }

    #[test]
    fn story_note_scans_for_unindexed_notes() {
        let tmp = tempfile::tempdir().unwrap();
        let notes = tempfile::tempdir().unwrap();
        let conn = open(tmp.path()).unwrap();
        assert_eq!(story_note(&conn, notes.path(), 7.into()).unwrap(), None);

        std::fs::create_dir_all(notes.path().join("stories")).unwrap();
        std::fs::write(notes.path().join("stories/some-story.md"), "unindexed").unwrap();
        std::fs::write(
            notes.path().join("stories/a-other.md"),
            "---\nid: a\nstory_id: sc-70\n---\n",
        )
        .unwrap();
        assert_eq!(story_note(&conn, notes.path(), 7.into()).unwrap(), None);

        let linked = "---\nid: some-story-7\nstory_id: sc-7\n---\nunindexed";
        std::fs::write(notes.path().join("stories/some-story-7.md"), linked).unwrap();
        assert_eq!(
            story_note(&conn, notes.path(), 7.into()).unwrap(),
            Some((rel("stories/some-story-7.md"), linked.into()))
        );

        std::fs::write(notes.path().join("other.md"), "indexed").unwrap();
        reindex_note(&conn, &rel("other.md"), "---\nid: o\nstory_id: sc-7\n---\n").unwrap();
        assert_eq!(
            story_note(&conn, notes.path(), 7.into()).unwrap(),
            Some((rel("other.md"), "indexed".into()))
        );
    }

    #[test]
    fn read_note_by_path() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("stories")).unwrap();
        std::fs::write(tmp.path().join("stories/a.md"), "hi").unwrap();
        assert_eq!(
            read_note(tmp.path(), &rel("stories/a.md")).unwrap(),
            Some("hi".into())
        );
        assert_eq!(read_note(tmp.path(), &rel("missing.md")).unwrap(), None);
    }

    #[test]
    fn query_capped_truncates() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = open(tmp.path()).unwrap();
        for i in 0..3 {
            reindex_note(&conn, &rel(&format!("{i}.md")), "---\nid: x\n---\n").unwrap();
        }
        let (rows, truncated) = select_capped(&conn, "SELECT path FROM notes", 2).unwrap();
        assert!(truncated);
        assert_eq!(
            serde_json::from_str::<Vec<serde_json::Value>>(&rows)
                .unwrap()
                .len(),
            2
        );
        let (_, truncated) = select_capped(&conn, "SELECT path FROM notes", 3).unwrap();
        assert!(!truncated);
        assert!(select_capped(&conn, "DELETE FROM notes", 2).is_err());
        assert!(select_capped(&conn, "WITH x AS (SELECT 1) DELETE FROM notes", 2).is_err());
        assert!(select_capped(&conn, "PRAGMA journal_mode = DELETE", 2).is_err());
        assert!(select_capped(&conn, "PRAGMA table_info(notes)", 100).is_ok());
    }

    #[test]
    fn read_only_sql_parse_rejects_writes() {
        for write in [
            "DELETE FROM notes",
            "  insert into notes(path) values ('x')",
            "UPDATE notes SET id = 1",
            "DROP TABLE notes",
            "",
        ] {
            let err = ReadOnlySql::parse(write).unwrap_err().to_string();
            assert_eq!(
                err,
                "only read-only queries (SELECT/WITH/EXPLAIN/PRAGMA) are allowed",
                "{write}"
            );
        }
        for read in [
            "SELECT 1",
            "with x as (select 1) select * from x",
            "EXPLAIN SELECT 1",
            "pragma table_info(notes)",
        ] {
            assert!(ReadOnlySql::parse(read).is_ok(), "{read}");
        }
    }

    #[test]
    fn runaway_query_times_out() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = open(tmp.path()).unwrap();
        let sql = "WITH RECURSIVE r(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM r)
                   SELECT count(*) FROM r";
        let start = std::time::Instant::now();
        let sql = ReadOnlySql::parse(sql).unwrap();
        let err = query_with_timeout(&conn, &sql, 10, std::time::Duration::from_millis(200))
            .unwrap_err();
        assert!(err.to_string().contains("timed out"), "{err}");
        assert!(start.elapsed() < std::time::Duration::from_secs(3));
        assert!(select(&conn, "SELECT 1").is_ok());
    }

    #[test]
    fn reindex_nests_in_callers_transaction() {
        let tmp = tempfile::tempdir().unwrap();
        let mut conn = open(tmp.path()).unwrap();
        let tx = conn.transaction().unwrap();
        reindex_note(&tx, &rel("a.md"), "---\nid: a\nbranches: [b]\n---\n").unwrap();
        tx.rollback().unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM note_branches", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
    }

    fn sqlite_master_rows(conn: &Connection) -> Vec<(String, String, Option<String>)> {
        let mut stmt = conn
            .prepare("SELECT type, name, sql FROM sqlite_master ORDER BY name")
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    #[test]
    fn golden_index_schema_is_stable() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = open(tmp.path()).unwrap();
        let row = |kind: &str, name: &str, sql: Option<&str>| {
            (kind.to_string(), name.to_string(), sql.map(str::to_string))
        };
        assert_eq!(
            sqlite_master_rows(&conn),
            vec![
                row(
                    "index",
                    "idx_note_branches_branch",
                    Some("CREATE INDEX idx_note_branches_branch ON note_branches(branch)"),
                ),
                row(
                    "index",
                    "idx_note_branches_path",
                    Some("CREATE INDEX idx_note_branches_path ON note_branches(path)"),
                ),
                row(
                    "index",
                    "idx_note_tags_path",
                    Some("CREATE INDEX idx_note_tags_path ON note_tags(path)"),
                ),
                row(
                    "index",
                    "idx_notes_entity",
                    Some("CREATE INDEX idx_notes_entity ON notes(link_kind, entity_id)"),
                ),
                row(
                    "table",
                    "note_branches",
                    Some("CREATE TABLE note_branches (\n    path   TEXT NOT NULL,\n    branch TEXT NOT NULL\n)"),
                ),
                row(
                    "table",
                    "note_tags",
                    Some("CREATE TABLE note_tags (\n    path TEXT NOT NULL,\n    tag  TEXT NOT NULL\n)"),
                ),
                row(
                    "table",
                    "notes",
                    Some("CREATE TABLE notes (\n    path          TEXT PRIMARY KEY,\n    id            TEXT,\n    title         TEXT,\n    created       TEXT,\n    type          TEXT,\n    link_kind     TEXT,\n    entity_id     INTEGER,\n    entity_url    TEXT,\n    entity_name   TEXT,\n    iteration_url TEXT,\n    epic_url      TEXT\n)"),
                ),
                row("index", "sqlite_autoindex_notes_1", None),
                row("index", "sqlite_autoindex_todos_1", None),
                row(
                    "table",
                    "todos",
                    Some("CREATE TABLE todos (\n    id        TEXT PRIMARY KEY,\n    text      TEXT,\n    date      TEXT,\n    completed INTEGER,\n    file      TEXT,\n    line      INTEGER,\n    position  INTEGER\n)"),
                ),
            ]
        );
        let journal_mode: String = conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(journal_mode, "wal");
    }

    #[test]
    fn golden_index_rows_are_stable() {
        let tmp = tempfile::tempdir().unwrap();
        let mut conn = open(tmp.path()).unwrap();
        reindex_note(&conn, &rel("stories/sc-7.md"), "---\nid: sc-7\ntags: [t]\ncreated: 2026-03-31\ntype: story\nstory_id: sc-7\nstory_link: https://s/7\nstory_name: Seven\niteration: https://it/3\nepic: https://ep/40\nbranches: [sc-7-branch]\n---\n\n# Title seven\n").unwrap();
        reindex_note(&conn, &rel("iterations/it-3.md"), "---\nid: it-3\niteration_id: it-3\niteration_link: https://it/3\niteration_name: Sprint 3\n---\n").unwrap();
        reindex_note(&conn, &rel("epics/ep-40.md"), "---\nid: ep-40\nepic_id: ep-40\nepic_link: https://ep/40\nepic_name: Forty\n---\n").unwrap();
        reindex_note(&conn, &rel("scratch/plain.md"), "# Plain heading\nno frontmatter\n").unwrap();
        mirror_todos(&mut conn, &[
            Todo {
                id: uuid::Uuid::from_u128(1),
                text: "added".into(),
                date: NaiveDate::from_ymd_opt(2026, 5, 31),
                completed: false,
                file: rel("todos.md"),
                line: 4,
                scope: Default::default(),
            },
            Todo {
                id: uuid::Uuid::from_u128(2),
                text: "parsed".into(),
                date: None,
                completed: true,
                file: rel("stories/sc-7.md"),
                line: 12,
                scope: Default::default(),
            },
        ]).unwrap();
        let rows = |sql: &str| -> serde_json::Value { serde_json::from_str(&select(&conn, sql).unwrap()).unwrap() };
        assert_eq!(
            rows("SELECT * FROM notes ORDER BY path"),
            serde_json::json!([
                {"path": "epics/ep-40.md", "id": "ep-40", "title": "ep-40", "created": null, "type": null, "link_kind": "epic", "entity_id": 40, "entity_url": "https://ep/40", "entity_name": "Forty", "iteration_url": null, "epic_url": null},
                {"path": "iterations/it-3.md", "id": "it-3", "title": "it-3", "created": null, "type": null, "link_kind": "iteration", "entity_id": 3, "entity_url": "https://it/3", "entity_name": "Sprint 3", "iteration_url": null, "epic_url": null},
                {"path": "scratch/plain.md", "id": null, "title": "Plain heading", "created": null, "type": null, "link_kind": null, "entity_id": null, "entity_url": null, "entity_name": null, "iteration_url": null, "epic_url": null},
                {"path": "stories/sc-7.md", "id": "sc-7", "title": "Title seven", "created": "2026-03-31", "type": "story", "link_kind": "story", "entity_id": 7, "entity_url": "https://s/7", "entity_name": "Seven", "iteration_url": "https://it/3", "epic_url": "https://ep/40"}
            ])
        );
        assert_eq!(
            rows("SELECT * FROM note_tags"),
            serde_json::json!([{"path": "stories/sc-7.md", "tag": "t"}])
        );
        assert_eq!(
            rows("SELECT * FROM note_branches"),
            serde_json::json!([{"path": "stories/sc-7.md", "branch": "sc-7-branch"}])
        );
        assert_eq!(
            rows("SELECT * FROM todos ORDER BY position"),
            serde_json::json!([
                {"id": "00000000-0000-0000-0000-000000000001", "text": "added", "date": "2026-05-31", "completed": 0, "file": "todos.md", "line": 4, "position": 0},
                {"id": "00000000-0000-0000-0000-000000000002", "text": "parsed", "date": null, "completed": 1, "file": "stories/sc-7.md", "line": 12, "position": 1}
            ])
        );
    }
}
