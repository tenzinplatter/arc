use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use chrono::NaiveDate;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerInfo};
use rmcp::schemars::{self, JsonSchema};
use rmcp::transport::stdio;
use rmcp::{ErrorData, ServerHandler, ServiceExt, tool, tool_handler, tool_router};
use serde::Deserialize;
use serde_json::{Value, json};
use slugify::slugify;
use tokio::sync::OnceCell;
use uuid::Uuid;

use arc_context::NoteLinks;
use arc_core::{Config, StoryId};
use arc_index::Connection;
use arc_notes::note::frontmatter::Frontmatter;
use arc_notes::NoteRelPath;
use arc_notes::todos::{Todo, TodoSource};
use arc_shortcut::api::branch::Branch;
use arc_shortcut::api::story::Story;
use arc_shortcut::{ApiClient, Cache};

const QUERY_ROW_CAP: usize = 200;
const NET_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Deserialize, JsonSchema)]
struct QueryArgs {
    /// A read-only SQL statement (SELECT/WITH/EXPLAIN/PRAGMA).
    sql: String,
}

#[derive(Deserialize, JsonSchema)]
struct StoryIdArgs {
    /// Shortcut story id.
    story_id: i32,
}

#[derive(Deserialize, JsonSchema)]
struct GetNoteArgs {
    /// The note's Obsidian `id` (frontmatter `id`, usually the filename slug).
    #[serde(default)]
    id: Option<String>,
    /// Notes-dir-relative path (e.g. `stories/foo.md`). Takes precedence over `id`.
    #[serde(default)]
    path: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct AppendNoteArgs {
    /// Obsidian `id` of an existing note to append to.
    #[serde(default)]
    id: Option<String>,
    /// Notes-dir-relative path of the note. Takes precedence over `id`.
    #[serde(default)]
    path: Option<String>,
    /// Markdown appended below the note's current content.
    content: String,
}

#[derive(Deserialize, JsonSchema)]
struct CreateNoteArgs {
    /// Human title; also the `# ` heading. The slug becomes the note id/filename.
    title: String,
    /// Markdown body placed under the heading.
    body: String,
    /// Optional frontmatter tags.
    #[serde(default)]
    tags: Vec<String>,
}

#[derive(Deserialize, JsonSchema)]
struct CreateStoryNoteArgs {
    /// Shortcut story id. The note is linked to this story.
    story_id: i32,
    /// Markdown body placed under the heading.
    body: String,
}

#[derive(Deserialize, JsonSchema)]
struct AddTodoArgs {
    /// Todo text.
    text: String,
    /// Optional due date, `YYYY-MM-DD`.
    #[serde(default)]
    date: Option<String>,
}

#[derive(Clone)]
pub struct ArcMcp {
    cache_dir: PathBuf,
    notes_dir: PathBuf,
    api_token: String,
    client: Arc<OnceCell<ApiClient>>,
    lookups_refreshed: Arc<AtomicBool>,
}

macro_rules! tool_try {
    ($e:expr) => {
        match $e {
            Ok(v) => v,
            Err(e) => return Ok(fail(e)),
        }
    };
}

#[tool_router]
impl ArcMcp {
    pub fn new(cache_dir: PathBuf, notes_dir: PathBuf, api_token: String) -> Self {
        Self {
            cache_dir,
            notes_dir,
            api_token,
            client: Arc::new(OnceCell::new()),
            lookups_refreshed: Arc::new(AtomicBool::new(false)),
        }
    }

    #[tool(
        description = "List the stories the arc TUI has cached for the current iteration(s) as a JSON array of {id, name, state, type, url, iteration_id, epic_id, estimate, has_note}. The list may be stale; use the Shortcut MCP for live search. Use `get_story` for description/comments."
    )]
    async fn active_stories(&self) -> Result<CallToolResult, ErrorData> {
        let mut cache = self.read_cache().await;
        let note = self
            .lookups(&mut cache, |c| {
                c.iteration_stories
                    .iter()
                    .flatten()
                    .any(|s| s.workflow_state_id.is_some() && c.state_name(s.workflow_state_id).is_none())
            })
            .await;
        let cache_dir = self.cache_dir.clone();
        let json = tokio::task::spawn_blocking(move || -> Result<String> {
            let conn = arc_index::open_readonly(&cache_dir)?;
            Ok(slim_stories(&cache, &conn)?.to_string())
        })
        .await
        .map_err(internal)?;
        Ok(with_notes(tool_try!(json), note))
    }

    #[tool(
        description = "Full detail of one Shortcut story as JSON: description, state, owners, branches, comments (author, created_at, text) and the linked note path. Fetches the story from Shortcut on each call, falling back to the cached copy if Shortcut is unreachable; works for stories outside the current iteration."
    )]
    async fn get_story(
        &self,
        Parameters(StoryIdArgs { story_id }): Parameters<StoryIdArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let story_id = StoryId::from(story_id);
        let mut cache = self.read_cache().await;
        let refresh_note = self
            .refresh_story(&mut cache, story_id)
            .await
            .err()
            .map(|e| format!("note: couldn't refresh from Shortcut ({e:#}); showing cached copy"));
        let Some(story) = cache.find_story(story_id).cloned() else {
            return Ok(fail(format!(
                "story {story_id} isn't cached and couldn't be fetched{}",
                refresh_note.map(|n| format!(" — {n}")).unwrap_or_default()
            )));
        };
        let lookup_note = self.lookups(&mut cache, |c| lookup_miss(c, &story)).await;
        let cache_dir = self.cache_dir.clone();
        let json = tokio::task::spawn_blocking(move || -> Result<String> {
            let conn = arc_index::open_readonly(&cache_dir)?;
            let note = arc_index::note_for_story(&conn, story_id)?.map(|p| p.to_string());
            Ok(story_detail(&cache, &story, note).to_string())
        })
        .await
        .map_err(internal)?;
        Ok(with_notes(tool_try!(json), refresh_note.into_iter().chain(lookup_note)))
    }

    #[tool(
        description = "Return the path and full Markdown of the note linked to a Shortcut story. Errors if the story has no note — create one with `create_story_note`."
    )]
    async fn get_story_note(
        &self,
        Parameters(StoryIdArgs { story_id }): Parameters<StoryIdArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let story_id = StoryId::from(story_id);
        let (cache_dir, notes_dir) = (self.cache_dir.clone(), self.notes_dir.clone());
        let found = tokio::task::spawn_blocking(move || -> Result<Option<(NoteRelPath, String)>> {
            let conn = arc_index::open_readonly(&cache_dir)?;
            arc_index::story_note(&conn, &notes_dir, story_id)
        })
        .await
        .map_err(internal)?;
        match tool_try!(found) {
            Some((path, body)) => Ok(CallToolResult::success(vec![
                ContentBlock::text(format!("path: {path}")),
                ContentBlock::text(body),
            ])),
            None => Ok(fail(format!("no note for story {story_id}; use create_story_note"))),
        }
    }

    #[tool(
        description = "Run a read-only SQL query against the notes index and return rows as a JSON array of objects (at most 200 rows). Tables: notes(path, id, title, created, type, link_kind['story'|'iteration'|'epic'|null], entity_id, entity_url, entity_name, iteration_url, epic_url); note_tags(path, tag); note_branches(path, branch); todos(id, text, date, completed, source['manual'|'note'], file, line, position). Only SELECT/WITH/EXPLAIN/PRAGMA are permitted."
    )]
    async fn query(
        &self,
        Parameters(QueryArgs { sql }): Parameters<QueryArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let cache_dir = self.cache_dir.clone();
        let rows_truncated = tokio::task::spawn_blocking(move || -> Result<(String, bool)> {
            let conn = arc_index::open_readonly(&cache_dir)?;
            arc_index::query_capped(&conn, &arc_index::ReadOnlySql::parse(&sql)?, QUERY_ROW_CAP)
        })
        .await
        .map_err(internal)?;
        let (rows, truncated) = tool_try!(rows_truncated);
        let note = truncated
            .then(|| format!("results truncated at {QUERY_ROW_CAP} rows; narrow the query or add LIMIT/OFFSET"));
        Ok(with_notes(rows, note))
    }

    #[tool(
        description = "Return the full Markdown body of a note by its notes-dir-relative `path` or its Obsidian `id` (the columns from the notes table). Errors listing candidate paths if the id is ambiguous."
    )]
    async fn get_note(
        &self,
        Parameters(GetNoteArgs { id, path }): Parameters<GetNoteArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let (cache_dir, notes_dir) = (self.cache_dir.clone(), self.notes_dir.clone());
        let body = tokio::task::spawn_blocking(move || -> Result<Option<String>> {
            let conn = arc_index::open_readonly(&cache_dir)?;
            let rel = resolve_note(&conn, NoteRef::from_args(id, path)?)?;
            arc_index::read_note(&notes_dir, &rel)
        })
        .await
        .map_err(internal)?;
        match tool_try!(body) {
            Some(text) => Ok(CallToolResult::success(vec![ContentBlock::text(text)])),
            None => Ok(fail("note not found")),
        }
    }

    #[tool(
        description = "Append Markdown to an existing note (by notes-dir-relative `path`, or Obsidian `id`). Use this to persist context or progress into a note. Non-destructive — content is added below what's already there."
    )]
    async fn append_note(
        &self,
        Parameters(AppendNoteArgs { id, path, content }): Parameters<AppendNoteArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let (cache_dir, notes_dir) = (self.cache_dir.clone(), self.notes_dir.clone());
        let rel = tokio::task::spawn_blocking(move || -> Result<NoteRelPath> {
            let conn = arc_index::open(&cache_dir)?;
            let rel = resolve_note(&conn, NoteRef::from_args(id, path)?)?;
            append_note(&conn, &notes_dir, &rel, &content)?;
            Ok(rel)
        })
        .await
        .map_err(internal)?;
        Ok(CallToolResult::success(vec![ContentBlock::text(format!(
            "appended to {}",
            tool_try!(rel)
        ))]))
    }

    #[tool(
        description = "Create a new general note (no Shortcut link). Returns the new note's `id` and notes-dir-relative `path`; pass that `path` to `append_note` to add to it later. Errors if a note with the same slug already exists — append_note to add to it instead."
    )]
    async fn create_note(
        &self,
        Parameters(CreateNoteArgs { title, body, tags }): Parameters<CreateNoteArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let (cache_dir, notes_dir) = (self.cache_dir.clone(), self.notes_dir.clone());
        let created = tokio::task::spawn_blocking(move || {
            let conn = arc_index::open(&cache_dir)?;
            create_note(&conn, &notes_dir, &title, &body, tags)
        })
        .await
        .map_err(internal)?;
        Ok(created_result(tool_try!(created)))
    }

    #[tool(
        description = "Create a note linked to a Shortcut story (by story id; fetched from Shortcut if it isn't cached). The note lives under `stories/`, its slug/title come from the story name, and it carries the story link so it shows up against that story. Returns the note's `id` and notes-dir-relative `path`; pass that `path` to `append_note` to add to it later. Errors if the story's note already exists — use `append_note` to add to it."
    )]
    async fn create_story_note(
        &self,
        Parameters(CreateStoryNoteArgs { story_id, body }): Parameters<CreateStoryNoteArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let story_id = StoryId::from(story_id);
        let mut cache = self.read_cache().await;
        if cache.find_story(story_id).is_none()
            && let Err(e) = self.refresh_story(&mut cache, story_id).await
        {
            return Ok(fail(format!(
                "story {story_id} isn't cached and couldn't be fetched: {e:#}"
            )));
        }
        let Some(story) = cache.find_story(story_id).cloned() else {
            return Ok(fail(format!("story {story_id} isn't cached and couldn't be fetched")));
        };
        let links = NoteLinks::for_story(&cache, &story);

        let (cache_dir, notes_dir) = (self.cache_dir.clone(), self.notes_dir.clone());
        let created = tokio::task::spawn_blocking(move || {
            let conn = arc_index::open(&cache_dir)?;
            arc_context::create_story_note(&conn, &notes_dir, &story, links, &body)
        })
        .await
        .map_err(internal)?;
        Ok(created_result(tool_try!(created)))
    }

    #[tool(
        description = "Add a manual todo. Shows up in the arc TUI and is indexed for querying. Optional due date as YYYY-MM-DD."
    )]
    async fn add_todo(
        &self,
        Parameters(AddTodoArgs { text, date }): Parameters<AddTodoArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let date = match date.as_deref() {
            Some(d) => Some(tool_try!(
                NaiveDate::parse_from_str(d, "%Y-%m-%d").map_err(|e| format!("bad date: {e}"))
            )),
            None => None,
        };
        let todo = Todo {
            id: Uuid::new_v4(),
            text,
            date,
            completed: false,
            source: TodoSource::Manual,
        };
        let updated = tool_try!(
            arc_notes::todos::transform_with_lock(&self.cache_dir, move |list| {
                list.into_iter().chain([todo]).collect()
            })
            .await
        );

        let cache_dir = self.cache_dir.clone();
        let mirrored = tokio::task::spawn_blocking(move || -> Result<()> {
            let mut conn = arc_index::open(&cache_dir)?;
            arc_index::mirror_todos(&mut conn, &updated)
        })
        .await
        .map_err(internal)?;
        tool_try!(mirrored);
        Ok(CallToolResult::success(vec![ContentBlock::text("added")]))
    }
}

impl ArcMcp {
    async fn read_cache(&self) -> Cache {
        // a missing cache file defaults to ~/.cache/arc; writes must go to ours
        Cache::read(self.cache_dir.clone())
            .await
            .with_cache_dir(self.cache_dir.clone())
    }

    async fn client(&self, cache: &mut Cache) -> Result<ApiClient> {
        self.client
            .get_or_try_init(|| timed(ApiClient::from_token_and_cache(&self.api_token, cache)))
            .await
            .cloned()
    }

    async fn lookups(&self, cache: &mut Cache, missing: impl Fn(&Cache) -> bool) -> Option<String> {
        if !missing(cache) {
            return None;
        }
        let res = async {
            let client = self.client(cache).await?;
            timed(arc_shortcut::ensure_lookups(cache, &client)).await?;
            if missing(cache) && !self.lookups_refreshed.swap(true, Ordering::Relaxed) {
                timed(arc_shortcut::refresh_lookups(cache, &client)).await?;
            }
            anyhow::Ok(())
        }
        .await;
        res.err()
            .map(|e| format!("note: couldn't load state/member names from Shortcut ({e:#})"))
    }

    async fn refresh_story(&self, cache: &mut Cache, id: StoryId) -> Result<()> {
        let client = self.client(cache).await?;
        let story = timed(client.get_story(id)).await?;
        *cache = self.read_cache().await;
        cache.upsert_story(story);
        cache.write().await
    }
}

async fn timed<T>(fut: impl Future<Output = Result<T>>) -> Result<T> {
    tokio::time::timeout(NET_TIMEOUT, fut)
        .await
        .map_err(|_| anyhow!("timed out after {}s", NET_TIMEOUT.as_secs()))?
}

fn internal(e: impl std::fmt::Display) -> ErrorData {
    ErrorData::internal_error(e.to_string(), None)
}

fn fail(e: impl std::fmt::Display) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(e.to_string())])
}

fn created_result((id, path): (String, NoteRelPath)) -> CallToolResult {
    CallToolResult::success(vec![ContentBlock::text(format!("id: {id}\npath: {path}"))])
}

fn with_notes(text: String, notes: impl IntoIterator<Item = String>) -> CallToolResult {
    CallToolResult::success(
        std::iter::once(text)
            .chain(notes)
            .map(ContentBlock::text)
            .collect(),
    )
}

fn lookup_miss(cache: &Cache, s: &Story) -> bool {
    (s.workflow_state_id.is_some() && cache.state_name(s.workflow_state_id).is_none())
        || s.owner_ids
            .iter()
            .chain(s.comments.iter().filter_map(|c| c.author_id.as_ref()))
            .any(|id| cache.member_name(id).is_none())
}

fn slim_stories(cache: &Cache, conn: &Connection) -> Result<Value> {
    cache
        .iteration_stories
        .iter()
        .flatten()
        .map(|s| {
            Ok(json!({
                "id": s.id,
                "name": s.name,
                "state": cache.state_name(s.workflow_state_id),
                "type": s.story_type,
                "url": s.app_url,
                "iteration_id": s.iteration_id,
                "epic_id": s.epic_id,
                "estimate": s.estimate,
                "has_note": arc_index::note_for_story(conn, s.id)?.is_some(),
            }))
        })
        .collect::<Result<Vec<_>>>()
        .map(Value::Array)
}

fn story_detail(cache: &Cache, s: &Story, note: Option<String>) -> Value {
    let owners: Vec<String> = s
        .owner_ids
        .iter()
        .map(|id| cache.member_name(id).map_or_else(|| id.to_string(), str::to_owned))
        .collect();
    let branches: Vec<&str> = s.branches.iter().map(Branch::name).collect();
    let mut comments: Vec<_> = s.comments.iter().filter(|c| !c.deleted).collect();
    comments.sort_by_key(|c| c.position);
    let comments: Vec<Value> = comments
        .into_iter()
        .map(|c| {
            json!({
                "author": c.author_id.map(|id| {
                    cache.member_name(&id).map_or_else(|| id.to_string(), str::to_owned)
                }),
                "created_at": c.created_at,
                "text": c.text,
            })
        })
        .collect();
    json!({
        "id": s.id,
        "name": s.name,
        "state": cache.state_name(s.workflow_state_id),
        "type": s.story_type,
        "url": s.app_url,
        "completed": s.completed,
        "iteration_id": s.iteration_id,
        "epic_id": s.epic_id,
        "estimate": s.estimate,
        "owners": owners,
        "branches": branches,
        "description": s.description,
        "comments": comments,
        "note": note,
    })
}

enum NoteRef {
    Path(NoteRelPath),
    Id(String),
}

impl NoteRef {
    fn from_args(id: Option<String>, path: Option<String>) -> Result<Self> {
        match (path, id) {
            (Some(path), _) => Ok(Self::Path(NoteRelPath::parse(&path)?)),
            (None, Some(id)) => Ok(Self::Id(id)),
            (None, None) => bail!("pass `id` or `path`"),
        }
    }
}

fn resolve_note(conn: &Connection, note: NoteRef) -> Result<NoteRelPath> {
    let id = match note {
        NoteRef::Path(rel) => return Ok(rel),
        NoteRef::Id(id) => id,
    };
    match arc_index::note_paths_for_id(conn, &id)?.as_slice() {
        [] => bail!("no note with id '{id}'"),
        [only] => Ok(only.clone()),
        several => bail!(
            "id '{id}' matches several notes; pass `path` instead: {}",
            several.iter().map(NoteRelPath::as_str).collect::<Vec<_>>().join(", ")
        ),
    }
}

fn append_note(conn: &Connection, notes_dir: &Path, rel: &NoteRelPath, content: &str) -> Result<()> {
    let existing = arc_index::read_note(notes_dir, rel)?
        .ok_or_else(|| anyhow!("no note at '{rel}'"))?;
    let separator = if existing.ends_with('\n') { "\n" } else { "\n\n" };
    let body = format!("{existing}{separator}{}\n", content.trim_end());
    std::fs::write(rel.abs(notes_dir), &body)?;
    arc_index::reindex_note(conn, rel, &body)?;
    Ok(())
}

fn create_note(
    conn: &Connection,
    notes_dir: &Path,
    title: &str,
    body: &str,
    tags: Vec<String>,
) -> Result<(String, NoteRelPath)> {
    let slug = slugify!(title);
    if slug.is_empty() {
        bail!("title produced an empty slug");
    }
    let rel = NoteRelPath::parse(&format!("{slug}.md"))?;
    let abs = rel.abs(notes_dir);
    if abs.exists() {
        bail!("note '{slug}' already exists; use append_note");
    }
    let content = format!(
        "{}\n# {title}\n\n{body}\n",
        Frontmatter::general(slug.clone(), tags).to_block()?
    );
    if let Some(p) = abs.parent() {
        std::fs::create_dir_all(p)?;
    }
    std::fs::write(&abs, &content)?;
    arc_index::reindex_note(conn, &rel, &content)?;
    Ok((slug, rel))
}

#[tool_handler]
impl ServerHandler for ArcMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("arc", env!("CARGO_PKG_VERSION")))
            .with_instructions(
            "arc: the notes vault plus a cache of Shortcut stories. Stories: \
             `active_stories` (slim list of the current iterations), `get_story` (full \
             detail incl. description and comments), `get_story_note` (a story's note). \
             Notes: `query` (read-only SQL over notes, tags, branches, todos), `get_note` \
             (full Markdown by path or id), `append_note` to add context to a note, \
             `create_note` for a new note, `create_story_note` for a note linked to a \
             story, `add_todo` for a todo. `active_stories` lists what the arc TUI last \
             cached for the current iteration(s) and may be stale (use the Shortcut MCP \
             for live search); `get_story` refetches the story from Shortcut, falling back \
             to the cache when it's unreachable. Shortcut writes (creating stories, state changes, \
             comments) aren't handled here — use the Shortcut MCP.",
            )
    }
}

pub async fn run_stdio(config: &Config) -> Result<()> {
    // created up front so read-only opens work before the daemon's first run
    arc_index::open(&config.cache_dir)?;

    let server = ArcMcp::new(
        config.cache_dir.clone(),
        config.notes_dir.clone(),
        config.api_token.clone(),
    );
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use arc_shortcut::api::story::comment::StoryComment;
    use arc_shortcut::api::user::MemberId;

    fn rel(s: &str) -> NoteRelPath {
        NoteRelPath::parse(s).unwrap()
    }

    fn resolve(conn: &Connection, id: Option<&str>, path: Option<&str>) -> Result<NoteRelPath> {
        resolve_note(conn, NoteRef::from_args(id.map(Into::into), path.map(Into::into))?)
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

    #[test]
    fn create_and_append_note() {
        let (_c, notes, conn) = setup();
        let created = create_note(&conn, notes.path(), "My Note", "body", vec!["t".into()]).unwrap();
        assert_eq!(created, ("my-note".into(), rel("my-note.md")));
        assert!(create_note(&conn, notes.path(), "My Note", "x", vec![]).is_err());

        let rel = resolve(&conn, Some("my-note"), None).unwrap();
        assert_eq!(rel, "my-note.md");
        append_note(&conn, notes.path(), &rel, "more\n\n").unwrap();
        let body = std::fs::read_to_string(notes.path().join(&rel)).unwrap();
        assert!(body.ends_with("body\n\nmore\n"));
        assert!(append_note(&conn, notes.path(), &NoteRelPath::parse("missing.md").unwrap(), "x").is_err());
    }

    #[test]
    fn append_by_unnormalised_path_keeps_one_index_row() {
        let (_c, notes, conn) = setup();
        arc_context::create_story_note(&conn, notes.path(), &story(8, "X"), NoteLinks::default(), "b").unwrap();
        let rel = resolve(&conn, None, Some("./stories//x.md")).unwrap();
        assert_eq!(rel, "stories/x.md");
        append_note(&conn, notes.path(), &rel, "more").unwrap();
        let sql = arc_index::ReadOnlySql::parse("SELECT path FROM notes").unwrap();
        let rows = arc_index::query(&conn, &sql).unwrap();
        assert_eq!(rows, r#"[{"path":"stories/x.md"}]"#);
    }

    #[test]
    fn resolve_note_by_id_and_path() {
        let (_c, _n, conn) = setup();
        arc_index::reindex_note(&conn, &rel("foo.md"), "---\nid: foo\n---\n").unwrap();
        arc_index::reindex_note(&conn, &rel("stories/foo.md"), "---\nid: foo\n---\n")
            .unwrap();
        let err = resolve(&conn, Some("foo"), None).unwrap_err().to_string();
        assert!(err.contains("foo.md") && err.contains("stories/foo.md"), "{err}");
        assert_eq!(
            resolve(&conn, Some("foo"), Some("stories/foo.md")).unwrap(),
            "stories/foo.md"
        );
        assert!(resolve(&conn, None, Some("../x.md")).is_err());
        assert!(resolve(&conn, Some("nope"), None).is_err());
        assert!(resolve(&conn, None, None).is_err());
    }

    #[test]
    fn slim_stories_shape() {
        let (_c, _n, conn) = setup();
        arc_index::reindex_note(&conn, &rel("stories/a.md"), "---\nid: a\nstory_id: sc-1\n---\n")
            .unwrap();
        let mut a = story(1, "A");
        a.workflow_state_id = Some(500.into());
        a.story_type = "feature".into();
        a.estimate = Some(3);
        a.description = "long".into();
        let mut cache = Cache::default();
        cache.iteration_stories = Some(vec![a, story(2, "B")]);
        cache.workflow_states.insert(500.into(), "In Progress".into());

        let v = slim_stories(&cache, &conn).unwrap();
        assert_eq!(
            v[0],
            json!({
                "id": 1, "name": "A", "state": "In Progress", "type": "feature",
                "url": "https://app.shortcut.com/x/story/1", "iteration_id": null,
                "epic_id": null, "estimate": 3, "has_note": true,
            })
        );
        assert_eq!(v[1]["state"], Value::Null);
        assert_eq!(v[1]["has_note"], false);
    }

    #[test]
    fn story_detail_names_and_comments() {
        let owner = MemberId::from(Uuid::from_u128(1));
        let unknown = MemberId::from(Uuid::from_u128(2));
        let mut s = story(3, "C");
        s.owner_ids = vec![owner, unknown];
        let comment = |pos: i32, deleted: bool, text: &str| StoryComment {
            id: pos.into(),
            author_id: Some(owner),
            created_at: None,
            deleted,
            position: pos,
            text: Some(text.into()),
        };
        s.comments = vec![comment(2, false, "second"), comment(1, false, "first"), comment(3, true, "gone")];
        let mut cache = Cache::default();
        cache.members.insert(owner, "tenzin".into());

        let v = story_detail(&cache, &s, Some("stories/c.md".into()));
        assert_eq!(v["owners"], json!(["tenzin", unknown.to_string()]));
        assert_eq!(v["comments"].as_array().unwrap().len(), 2);
        assert_eq!(v["comments"][0]["text"], "first");
        assert_eq!(v["comments"][0]["author"], "tenzin");
        assert_eq!(v["note"], "stories/c.md");
    }

    #[test]
    fn golden_created_result_text() {
        assert_eq!(
            serde_json::to_value(created_result(("sc-7-slug".into(), rel("stories/sc-7-slug.md")))).unwrap(),
            json!({
                "content": [{"type": "text", "text": "id: sc-7-slug\npath: stories/sc-7-slug.md"}],
                "isError": false
            })
        );
    }

    fn golden_detail_story() -> (Cache, Story) {
        let known = MemberId::from(uuid::Uuid::from_u128(1));
        let unknown = MemberId::from(uuid::Uuid::from_u128(2));
        let mut cache = Cache::default();
        cache.workflow_states.insert(500.into(), "In Progress".into());
        cache.members.insert(known, "tenzin".into());
        let comment = |id: i64, author_id, deleted, position, text: &str| StoryComment {
            id,
            author_id,
            created_at: Some("2026-01-06T10:00:00Z".parse().unwrap()),
            deleted,
            position,
            text: Some(text.into()),
        };
        let s = Story {
            branches: vec![serde_json::from_value(json!({"id": 11, "name": "sc-7-feature"})).unwrap()],
            completed: true,
            comments: vec![
                comment(3, Some(unknown), false, 2, "second"),
                comment(2, None, true, 1, "deleted"),
                comment(1, Some(known), false, 0, "first"),
            ],
            description: "desc".into(),
            epic_id: Some(40.into()),
            id: 7.into(),
            iteration_id: Some(3.into()),
            name: "story seven".into(),
            app_url: "https://app.shortcut.com/x/story/7".into(),
            workflow_state_id: Some(500.into()),
            story_type: "feature".into(),
            owner_ids: vec![known, unknown],
            estimate: Some(3),
        };
        (cache, s)
    }

    #[test]
    fn golden_story_detail_json() {
        let (cache, s) = golden_detail_story();
        assert_eq!(
            story_detail(&cache, &s, Some("note body".into())),
            json!({
                "id": 7,
                "name": "story seven",
                "state": "In Progress",
                "type": "feature",
                "url": "https://app.shortcut.com/x/story/7",
                "completed": true,
                "iteration_id": 3,
                "epic_id": 40,
                "estimate": 3,
                "owners": ["tenzin", "00000000-0000-0000-0000-000000000002"],
                "branches": ["sc-7-feature"],
                "description": "desc",
                "comments": [
                    {"author": "tenzin", "created_at": "2026-01-06T10:00:00Z", "text": "first"},
                    {"author": "00000000-0000-0000-0000-000000000002", "created_at": "2026-01-06T10:00:00Z", "text": "second"}
                ],
                "note": "note body"
            })
        );
    }

    #[test]
    fn golden_slim_stories_json() {
        let (_c, notes, conn) = setup();
        let (mut cache, s) = golden_detail_story();
        arc_context::create_story_note(&conn, notes.path(), &s, NoteLinks::default(), "hi").unwrap();
        cache.iteration_stories = Some(vec![s, story(8, "eight")]);
        assert_eq!(
            slim_stories(&cache, &conn).unwrap(),
            json!([
                {"id": 7, "name": "story seven", "state": "In Progress", "type": "feature", "url": "https://app.shortcut.com/x/story/7", "iteration_id": 3, "epic_id": 40, "estimate": 3, "has_note": true},
                {"id": 8, "name": "eight", "state": null, "type": "", "url": "https://app.shortcut.com/x/story/8", "iteration_id": null, "epic_id": null, "estimate": null, "has_note": false}
            ])
        );
    }
}
