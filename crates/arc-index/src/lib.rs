//! SQLite index of the notes vault — a queryable projection the daemon keeps
//! fresh. `todos.json` remains the authoritative todo store; this mirrors it
//! alongside note metadata + Shortcut links so the MCP server can answer SQL.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
pub use rusqlite::Connection;
use rusqlite::OpenFlags;

use arc_notes::note::frontmatter::{EntityLink, Frontmatter};
use arc_notes::todos::{Todo, TodoSource};

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
    source    TEXT,
    file      TEXT,
    line      INTEGER,
    position  INTEGER
);
CREATE INDEX IF NOT EXISTS idx_note_tags_path ON note_tags(path);
CREATE INDEX IF NOT EXISTS idx_note_branches_branch ON note_branches(branch);
CREATE INDEX IF NOT EXISTS idx_note_branches_path ON note_branches(path);
CREATE INDEX IF NOT EXISTS idx_notes_entity ON notes(link_kind, entity_id);
";

fn db_path(cache_dir: &Path) -> PathBuf {
    cache_dir.join(DB_FILE)
}

/// Open (creating + migrating) the writer connection. WAL so a concurrent
/// read-only reader (the MCP server) never blocks the daemon's writes.
pub fn open(cache_dir: &Path) -> Result<Connection> {
    std::fs::create_dir_all(cache_dir).ok();
    let conn = Connection::open(db_path(cache_dir))
        .with_context(|| format!("open index at {}", db_path(cache_dir).display()))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    // Daemon and the MCP writer can both hold writer connections; wait rather
    // than fail on the brief windows where they contend.
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

/// Open a read-only connection for querying. Errors if the index doesn't exist
/// yet (the daemon creates it on first run).
pub fn open_readonly(cache_dir: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(db_path(cache_dir), OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("open index (ro) at {}", db_path(cache_dir).display()))?;
    Ok(conn)
}

/// Upsert one note's frontmatter, tags + branches. `rel` is the notes-dir-relative path.
/// Atomic: readers never see the note half-written.
pub fn reindex_note(conn: &Connection, rel: &Path, content: &str) -> Result<()> {
    // A savepoint (not a transaction) so this nests inside a caller's transaction.
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

fn write_note_rows(conn: &Connection, rel: &Path, content: &str) -> Result<()> {
    let path = rel.to_string_lossy().to_string();
    let fm = Frontmatter::from_note(content);
    let title = note_title(content, fm.as_ref());

    let (id, created, note_type, link) = match &fm {
        Some(f) => (
            Some(f.id.clone()),
            f.created.map(|d| d.to_string()),
            f.note_type.clone(),
            &f.link,
        ),
        None => (None, None, None, &EntityLink::None),
    };

    let (kind, eid, eurl, ename, it_url, ep_url) = match link {
        EntityLink::None => (None, None, None, None, None, None),
        EntityLink::Story {
            id,
            url,
            name,
            iteration_url,
            epic_url,
        } => (
            Some("story"),
            Some(*id),
            Some(url.clone()),
            name.clone(),
            iteration_url.clone(),
            epic_url.clone(),
        ),
        EntityLink::Iteration { id, url, name } => (
            Some("iteration"),
            Some(*id),
            Some(url.clone()),
            name.clone(),
            None,
            None,
        ),
        EntityLink::Epic { id, url, name } => (
            Some("epic"),
            Some(*id),
            Some(url.clone()),
            name.clone(),
            None,
            None,
        ),
    };

    conn.execute(
        "INSERT OR REPLACE INTO notes
         (path, id, title, created, type, link_kind, entity_id, entity_url, entity_name, iteration_url, epic_url)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        rusqlite::params![path, id, title, created, note_type, kind, eid, eurl, ename, it_url, ep_url],
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

/// Drop a note (and its tags + branches) from the index.
pub fn remove_note(conn: &Connection, rel: &Path) -> Result<()> {
    let path = rel.to_string_lossy().to_string();
    conn.execute("DELETE FROM notes WHERE path = ?1", [&path])?;
    conn.execute("DELETE FROM note_tags WHERE path = ?1", [&path])?;
    conn.execute("DELETE FROM note_branches WHERE path = ?1", [&path])?;
    Ok(())
}

/// Replace the whole todos mirror. `todos.json` stays authoritative; this is a
/// read-only projection for querying, refreshed whenever the daemon has the
/// full list in hand.
pub fn mirror_todos(conn: &mut Connection, todos: &[Todo]) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM todos", [])?;
    for (pos, t) in todos.iter().enumerate() {
        let (source, file, line) = match &t.source {
            TodoSource::Manual => ("manual", None, None),
            TodoSource::NoteParsed { file, line, .. } => (
                "note",
                Some(file.to_string_lossy().to_string()),
                Some(*line),
            ),
        };
        tx.execute(
            "INSERT INTO todos (id, text, date, completed, source, file, line, position)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                t.id.to_string(),
                t.text,
                t.date.map(|d| d.to_string()),
                t.completed as i64,
                source,
                file,
                line,
                pos as i64,
            ],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// Run a read-only SELECT and return the rows as a JSON array of objects.
/// Rejects anything that isn't a read — belt to the read-only connection's
/// braces so a caller can't mutate even if handed a writer connection.
pub fn query(conn: &Connection, sql: &str) -> Result<String> {
    Ok(query_capped(conn, sql, usize::MAX)?.0)
}

/// [`query`] with at most `max_rows` rows. Returns the JSON array and whether
/// more rows were available. Aborts after [`QUERY_TIMEOUT`].
pub fn query_capped(conn: &Connection, sql: &str, max_rows: usize) -> Result<(String, bool)> {
    query_with_timeout(conn, sql, max_rows, QUERY_TIMEOUT)
}

const QUERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

fn query_with_timeout(
    conn: &Connection,
    sql: &str,
    max_rows: usize,
    timeout: std::time::Duration,
) -> Result<(String, bool)> {
    if !is_read_only(sql) {
        anyhow::bail!("only read-only queries (SELECT/WITH/EXPLAIN/PRAGMA) are allowed");
    }
    let deadline = std::time::Instant::now() + timeout;
    conn.progress_handler(1000, Some(move || std::time::Instant::now() > deadline))?;
    let result = run_query(conn, sql, max_rows);
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
    let mut rows = stmt.query([])?;
    let mut out: Vec<serde_json::Value> = Vec::new();
    let mut truncated = false;
    while let Some(row) = rows.next()? {
        if out.len() >= max_rows {
            truncated = true;
            break;
        }
        let mut obj = serde_json::Map::new();
        for (i, name) in cols.iter().enumerate() {
            obj.insert(name.clone(), value_to_json(row.get_ref(i)?));
        }
        out.push(serde_json::Value::Object(obj));
    }
    Ok((serde_json::to_string(&out)?, truncated))
}

/// Paths of every note with this Obsidian `id` — ids can collide across dirs.
pub fn note_paths_for_id(conn: &Connection, id: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT path FROM notes WHERE id = ?1 ORDER BY path")?;
    let paths = stmt
        .query_map([id], |r| r.get(0))?
        .collect::<rusqlite::Result<Vec<String>>>()?;
    Ok(paths)
}

/// Path of the note linked to a story, preferring one under `stories/`.
pub fn note_for_story(conn: &Connection, story_id: i32) -> Result<Option<String>> {
    let mut stmt = conn.prepare(
        "SELECT path FROM notes WHERE link_kind = 'story' AND entity_id = ?1
         ORDER BY path NOT LIKE 'stories/%', path LIMIT 1",
    )?;
    let mut rows = stmt.query_map([story_id], |r| r.get(0))?;
    Ok(rows.next().transpose()?)
}

/// `(story_id, path)` of the story note listing `branch` in its `branches:`.
pub fn story_for_branch(conn: &Connection, branch: &str) -> Result<Option<(i32, String)>> {
    let mut stmt = conn.prepare(
        "SELECT n.entity_id, n.path FROM note_branches b
         JOIN notes n ON n.path = b.path
         WHERE b.branch = ?1 AND n.link_kind = 'story'
         ORDER BY n.path NOT LIKE 'stories/%', n.path LIMIT 1",
    )?;
    let mut rows = stmt.query_map([branch], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.next().transpose()?)
}

/// Read a note by notes-dir-relative path. `None` if it doesn't exist. Errors
/// on absolute paths or `..` so untrusted paths can't escape `notes_dir`.
pub fn read_note(notes_dir: &Path, rel: &str) -> Result<Option<String>> {
    let escapes = Path::new(rel).components().any(|c| {
        !matches!(
            c,
            std::path::Component::Normal(_) | std::path::Component::CurDir
        )
    });
    if rel.is_empty() || escapes {
        anyhow::bail!("note path must be relative to the notes dir: {rel}");
    }
    match std::fs::read_to_string(notes_dir.join(rel)) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("read note {rel}")),
    }
}

fn is_read_only(sql: &str) -> bool {
    let head = sql.split_whitespace().next().unwrap_or("");
    matches!(
        head.to_ascii_uppercase().as_str(),
        "SELECT" | "WITH" | "EXPLAIN" | "PRAGMA"
    )
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

/// First `# ` heading, else the frontmatter id, else the file stem later.
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

    #[test]
    fn indexes_and_queries_a_story_note() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = open(tmp.path()).unwrap();
        let content = "---\nid: my-story\ntags: [backend]\ncreated: \"2026-03-31\"\nstory_id: sc-19559\nstory_link: https://s/19559\n---\n\n# Ship the thing\n\nbody";
        reindex_note(&conn, Path::new("stories/my-story.md"), content).unwrap();

        let rows = query(
            &conn,
            "SELECT id, title, entity_id, link_kind FROM notes WHERE link_kind = 'story'",
        )
        .unwrap();
        assert!(rows.contains("my-story"));
        assert!(rows.contains("Ship the thing"));
        assert!(rows.contains("19559"));

        let tags = query(&conn, "SELECT tag FROM note_tags").unwrap();
        assert!(tags.contains("backend"));
    }

    #[test]
    fn reindex_replaces_not_duplicates() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = open(tmp.path()).unwrap();
        let p = Path::new("a.md");
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
        reindex_note(&conn, Path::new("stories/a.md"), "---\nid: a\n---\n# Hi\n").unwrap();

        let todos = vec![Todo::new_manual(
            "do it".into(),
            NaiveDate::from_ymd_opt(2026, 5, 1).unwrap(),
        )];
        mirror_todos(&mut conn, &todos).unwrap();
        let got = query(&conn, "SELECT text FROM todos").unwrap();
        assert!(got.contains("do it"));

        let body = read_note(&notes, "stories/a.md").unwrap().unwrap();
        assert!(body.contains("# Hi"));

        assert!(query(&conn, "DELETE FROM todos").is_err());
    }

    #[test]
    fn indexes_branches_and_clears_on_remove() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = open(tmp.path()).unwrap();
        let p = Path::new("stories/s.md");
        let content = "---\nid: s\nstory_id: sc-42\nbranches: [me/sc-42/s, stacked]\n---\n";
        reindex_note(&conn, p, content).unwrap();
        assert_eq!(
            story_for_branch(&conn, "stacked").unwrap(),
            Some((42, "stories/s.md".into()))
        );
        assert_eq!(story_for_branch(&conn, "nope").unwrap(), None);

        // Reindex drops branches no longer listed.
        reindex_note(
            &conn,
            p,
            "---\nid: s\nstory_id: sc-42\nbranches: [stacked]\n---\n",
        )
        .unwrap();
        assert_eq!(story_for_branch(&conn, "me/sc-42/s").unwrap(), None);

        remove_note(&conn, p).unwrap();
        assert_eq!(story_for_branch(&conn, "stacked").unwrap(), None);
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM note_branches", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn branches_on_non_story_notes_dont_resolve() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = open(tmp.path()).unwrap();
        reindex_note(&conn, Path::new("x.md"), "---\nid: x\nbranches: [b]\n---\n").unwrap();
        assert_eq!(story_for_branch(&conn, "b").unwrap(), None);
    }

    #[test]
    fn note_for_story_prefers_stories_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = open(tmp.path()).unwrap();
        let fm = "---\nid: s\nstory_id: sc-7\n---\n";
        reindex_note(&conn, Path::new("archive/s.md"), fm).unwrap();
        reindex_note(&conn, Path::new("stories/s.md"), fm).unwrap();
        assert_eq!(
            note_for_story(&conn, 7).unwrap(),
            Some("stories/s.md".into())
        );
        assert_eq!(note_for_story(&conn, 8).unwrap(), None);
    }

    #[test]
    fn note_paths_for_colliding_ids() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = open(tmp.path()).unwrap();
        reindex_note(&conn, Path::new("stories/foo.md"), "---\nid: foo\n---\n").unwrap();
        reindex_note(&conn, Path::new("foo.md"), "---\nid: foo\n---\n").unwrap();
        assert_eq!(
            note_paths_for_id(&conn, "foo").unwrap(),
            vec!["foo.md".to_string(), "stories/foo.md".to_string()]
        );
        assert!(note_paths_for_id(&conn, "bar").unwrap().is_empty());
    }

    #[test]
    fn read_note_by_path() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("stories")).unwrap();
        std::fs::write(tmp.path().join("stories/a.md"), "hi").unwrap();
        assert_eq!(
            read_note(tmp.path(), "stories/a.md").unwrap(),
            Some("hi".into())
        );
        assert_eq!(read_note(tmp.path(), "missing.md").unwrap(), None);
        for bad in ["/etc/passwd", "../x.md", "stories/../../x.md", ""] {
            assert!(read_note(tmp.path(), bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn query_capped_truncates() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = open(tmp.path()).unwrap();
        for i in 0..3 {
            reindex_note(&conn, Path::new(&format!("{i}.md")), "---\nid: x\n---\n").unwrap();
        }
        let (rows, truncated) = query_capped(&conn, "SELECT path FROM notes", 2).unwrap();
        assert!(truncated);
        assert_eq!(
            serde_json::from_str::<Vec<serde_json::Value>>(&rows)
                .unwrap()
                .len(),
            2
        );
        let (_, truncated) = query_capped(&conn, "SELECT path FROM notes", 3).unwrap();
        assert!(!truncated);
        assert!(query_capped(&conn, "DELETE FROM notes", 2).is_err());
        assert!(query_capped(&conn, "WITH x AS (SELECT 1) DELETE FROM notes", 2).is_err());
        assert!(query_capped(&conn, "PRAGMA journal_mode = DELETE", 2).is_err());
        assert!(query_capped(&conn, "PRAGMA table_info(notes)", 100).is_ok());
    }

    #[test]
    fn runaway_query_times_out() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = open(tmp.path()).unwrap();
        let sql = "WITH RECURSIVE r(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM r)
                   SELECT count(*) FROM r";
        let start = std::time::Instant::now();
        let err =
            query_with_timeout(&conn, sql, 10, std::time::Duration::from_millis(200)).unwrap_err();
        assert!(err.to_string().contains("timed out"), "{err}");
        assert!(start.elapsed() < std::time::Duration::from_secs(3));
        // Handler is cleared: the connection still works.
        assert!(query(&conn, "SELECT 1").is_ok());
    }

    #[test]
    fn reindex_nests_in_callers_transaction() {
        let tmp = tempfile::tempdir().unwrap();
        let mut conn = open(tmp.path()).unwrap();
        let tx = conn.transaction().unwrap();
        reindex_note(&tx, Path::new("a.md"), "---\nid: a\nbranches: [b]\n---\n").unwrap();
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
                    Some("CREATE TABLE todos (\n    id        TEXT PRIMARY KEY,\n    text      TEXT,\n    date      TEXT,\n    completed INTEGER,\n    source    TEXT,\n    file      TEXT,\n    line      INTEGER,\n    position  INTEGER\n)"),
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
        reindex_note(&conn, Path::new("stories/sc-7.md"), "---\nid: sc-7\ntags: [t]\ncreated: 2026-03-31\ntype: story\nstory_id: sc-7\nstory_link: https://s/7\nstory_name: Seven\niteration: https://it/3\nepic: https://ep/40\nbranches: [sc-7-branch]\n---\n\n# Title seven\n").unwrap();
        reindex_note(&conn, Path::new("iterations/it-3.md"), "---\nid: it-3\niteration_id: it-3\niteration_link: https://it/3\niteration_name: Sprint 3\n---\n").unwrap();
        reindex_note(&conn, Path::new("epics/ep-40.md"), "---\nid: ep-40\nepic_id: ep-40\nepic_link: https://ep/40\nepic_name: Forty\n---\n").unwrap();
        reindex_note(&conn, Path::new("scratch/plain.md"), "# Plain heading\nno frontmatter\n").unwrap();
        mirror_todos(&mut conn, &[
            Todo {
                id: uuid::Uuid::from_u128(1),
                text: "manual".into(),
                date: NaiveDate::from_ymd_opt(2026, 5, 31),
                completed: false,
                source: TodoSource::Manual,
            },
            Todo {
                id: uuid::Uuid::from_u128(2),
                text: "parsed".into(),
                date: None,
                completed: true,
                source: TodoSource::NoteParsed { file: "stories/sc-7.md".into(), line: 12, fingerprint: 5 },
            },
        ]).unwrap();
        let rows = |sql: &str| -> serde_json::Value { serde_json::from_str(&query(&conn, sql).unwrap()).unwrap() };
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
                {"id": "00000000-0000-0000-0000-000000000001", "text": "manual", "date": "2026-05-31", "completed": 0, "source": "manual", "file": null, "line": null, "position": 0},
                {"id": "00000000-0000-0000-0000-000000000002", "text": "parsed", "date": null, "completed": 1, "source": "note", "file": "stories/sc-7.md", "line": 12, "position": 1}
            ])
        );
    }
}
