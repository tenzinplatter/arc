//! MCP server over the notes index (`arc.db`) and the Shortcut cache. Speaks
//! stdio, so an MCP client spawns `arc mcp` on demand; it reads the same index
//! the daemon maintains, so there's one source of truth and no cross-process
//! sync. Writes go to notes, todos and the Shortcut cache (`cache.json`, which
//! the TUI shares); the Shortcut API is only read, lazily, and every tool falls
//! back to cached data when it's unreachable.

use std::future::Future;
use std::path::{Component, Path, PathBuf};
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

use arc_core::Config;
use arc_index::Connection;
use arc_notes::note::frontmatter::{EntityLink, Frontmatter};
use arc_notes::todos::{Todo, TodoSource};
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
    // built on first network use
    client: Arc<OnceCell<ApiClient>>,
    // refresh_lookups runs at most once per process
    lookups_refreshed: Arc<AtomicBool>,
}

/// Unwraps a tool-level `Result`, returning its error as a failed tool call.
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
        let mut cache = self.read_cache().await;
        let mut notes = Vec::new();
        if let Err(e) = self.refresh_story(&mut cache, story_id).await {
            notes.push(format!("note: couldn't refresh from Shortcut ({e:#}); showing cached copy"));
        }
        let Some(story) = cache.find_story(story_id).cloned() else {
            return Ok(fail(format!(
                "story {story_id} isn't cached and couldn't be fetched{}",
                notes.first().map(|n| format!(" — {n}")).unwrap_or_default()
            )));
        };
        notes.extend(self.lookups(&mut cache, |c| lookup_miss(c, &story)).await);
        let cache_dir = self.cache_dir.clone();
        let json = tokio::task::spawn_blocking(move || -> Result<String> {
            let conn = arc_index::open_readonly(&cache_dir)?;
            let note = arc_index::note_for_story(&conn, story_id)?;
            Ok(story_detail(&cache, &story, note).to_string())
        })
        .await
        .map_err(internal)?;
        let mut content = vec![ContentBlock::text(tool_try!(json))];
        content.extend(notes.into_iter().map(ContentBlock::text));
        Ok(CallToolResult::success(content))
    }

    #[tool(
        description = "Return the path and full Markdown of the note linked to a Shortcut story. Errors if the story has no note — create one with `create_story_note`."
    )]
    async fn get_story_note(
        &self,
        Parameters(StoryIdArgs { story_id }): Parameters<StoryIdArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let (cache_dir, notes_dir) = (self.cache_dir.clone(), self.notes_dir.clone());
        let found = tokio::task::spawn_blocking(move || -> Result<Option<(String, String)>> {
            let conn = arc_index::open_readonly(&cache_dir)?;
            story_note(&conn, &notes_dir, story_id)
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
            arc_index::query_capped(&conn, &sql, QUERY_ROW_CAP)
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
            let rel = resolve_note(&conn, id.as_deref(), path.as_deref())?;
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
        let rel = tokio::task::spawn_blocking(move || -> Result<String> {
            let conn = arc_index::open(&cache_dir)?;
            let rel = resolve_note(&conn, id.as_deref(), path.as_deref())?;
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
        let iteration_url = story.iteration_id.and_then(|id| {
            cache
                .current_iterations
                .iter()
                .flatten()
                .chain(&cache.iterations)
                .find(|it| it.id == id)
                .map(|it| it.app_url.clone())
        });
        let epic_url = story.epic_id.and_then(|id| {
            cache
                .epics
                .iter()
                .find(|e| e.id == id)
                .map(|e| e.app_url.clone())
        });

        let (cache_dir, notes_dir) = (self.cache_dir.clone(), self.notes_dir.clone());
        let created = tokio::task::spawn_blocking(move || {
            let conn = arc_index::open(&cache_dir)?;
            create_story_note(&conn, &notes_dir, &story, iteration_url, epic_url, &body)
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
            arc_notes::todos::modify_with_lock(&self.cache_dir, move |list| list.push(todo)).await
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
        let mut cache = Cache::read(self.cache_dir.clone()).await;
        // a missing cache file defaults to ~/.cache/arc; writes must go to ours
        cache.cache_dir = self.cache_dir.clone();
        cache
    }

    async fn client(&self, cache: &mut Cache) -> Result<ApiClient> {
        self.client
            .get_or_try_init(|| timed(ApiClient::from_token_and_cache(&self.api_token, cache)))
            .await
            .cloned()
    }

    /// Best-effort fill of state/member names while `missing` reports a miss;
    /// refetches them once per process if a miss survives the initial fill.
    /// Returns a note on failure.
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

    /// Fetch a fresh copy of the story and persist it. The cache is re-read
    /// after the fetch so concurrent writers' changes aren't clobbered.
    async fn refresh_story(&self, cache: &mut Cache, id: i32) -> Result<()> {
        let client = self.client(cache).await?;
        let story = timed(client.get_story(id)).await?;
        *cache = self.read_cache().await;
        store_story(cache, story);
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

fn created_result((id, path): (String, String)) -> CallToolResult {
    CallToolResult::success(vec![ContentBlock::text(format!("id: {id}\npath: {path}"))])
}

/// `text` plus an optional trailing note block.
fn with_notes(text: String, note: Option<String>) -> CallToolResult {
    let mut content = vec![ContentBlock::text(text)];
    content.extend(note.map(ContentBlock::text));
    CallToolResult::success(content)
}

/// Replace the iteration copy in place, else upsert into `stories_by_id`; the
/// active story is refreshed too when it's the same one.
fn store_story(cache: &mut Cache, story: Story) {
    if let Some(active) = cache.active_story.as_mut().filter(|s| s.id == story.id) {
        *active = story.clone();
    }
    match cache
        .iteration_stories
        .iter_mut()
        .flatten()
        .find(|s| s.id == story.id)
    {
        Some(s) => *s = story,
        None => {
            cache.stories_by_id.insert(story.id, story);
        }
    }
}

/// Whether the story's state, an owner or a comment author has no cached name.
fn lookup_miss(cache: &Cache, s: &Story) -> bool {
    (s.workflow_state_id.is_some() && cache.state_name(s.workflow_state_id).is_none())
        || s.owner_ids
            .iter()
            .chain(s.comments.iter().filter_map(|c| c.author_id.as_ref()))
            .any(|id| cache.member_name(id).is_none())
}

/// Iteration stories as the slim `active_stories` payload.
fn slim_stories(cache: &Cache, conn: &Connection) -> Result<Value> {
    let stories = cache.iteration_stories.as_deref().unwrap_or_default();
    let mut out = Vec::with_capacity(stories.len());
    for s in stories {
        out.push(json!({
            "id": s.id,
            "name": s.name,
            "state": cache.state_name(s.workflow_state_id),
            "type": s.story_type,
            "url": s.app_url,
            "iteration_id": s.iteration_id,
            "epic_id": s.epic_id,
            "estimate": s.estimate,
            "has_note": arc_index::note_for_story(conn, s.id)?.is_some(),
        }));
    }
    Ok(Value::Array(out))
}

fn story_detail(cache: &Cache, s: &Story, note: Option<String>) -> Value {
    let owners: Vec<String> = s
        .owner_ids
        .iter()
        .map(|id| cache.member_name(id).map_or_else(|| id.to_string(), str::to_owned))
        .collect();
    // Branch fields are private; go through its serialized form
    let branches: Vec<Value> = serde_json::to_value(&s.branches)
        .ok()
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|b| b.get("name").cloned())
        .collect();
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

/// `(path, content)` of a story's note: the index first, then a scan of
/// `stories/` for notes the daemon hasn't indexed yet.
fn story_note(
    conn: &Connection,
    notes_dir: &Path,
    story_id: i32,
) -> Result<Option<(String, String)>> {
    if let Some(rel) = arc_index::note_for_story(conn, story_id)?
        && let Some(body) = arc_index::read_note(notes_dir, &rel)?
    {
        return Ok(Some((rel, body)));
    }
    scan_story_note(notes_dir, story_id)
}

/// First `stories/*.md` whose frontmatter links to the story.
fn scan_story_note(notes_dir: &Path, story_id: i32) -> Result<Option<(String, String)>> {
    let mut names: Vec<String> = match std::fs::read_dir(notes_dir.join("stories")) {
        Ok(dir) => dir
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter(|n| n.ends_with(".md"))
            .collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    names.sort();
    for name in names {
        let rel = format!("stories/{name}");
        if let Some(body) = arc_index::read_note(notes_dir, &rel)?
            && links_story(&body, story_id)
        {
            return Ok(Some((rel, body)));
        }
    }
    Ok(None)
}

fn links_story(content: &str, story_id: i32) -> bool {
    Frontmatter::from_note(content)
        .is_some_and(|fm| matches!(fm.link, EntityLink::Story { id, .. } if id == story_id))
}

/// Normalised notes-dir-relative path (`./` and repeated `/` dropped).
/// Rejects absolute paths and anything that could climb out of the notes dir.
fn check_rel_path(rel: &str) -> Result<String> {
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
    Ok(norm)
}

/// Notes-dir-relative path from an explicit `path`, else a unique `id`.
fn resolve_note(conn: &Connection, id: Option<&str>, path: Option<&str>) -> Result<String> {
    if let Some(rel) = path {
        return check_rel_path(rel);
    }
    let id = id.ok_or_else(|| anyhow!("pass `id` or `path`"))?;
    let mut paths = arc_index::note_paths_for_id(conn, id)?;
    match paths.len() {
        0 => bail!("no note with id '{id}'"),
        1 => Ok(paths.remove(0)),
        _ => bail!(
            "id '{id}' matches several notes; pass `path` instead: {}",
            paths.join(", ")
        ),
    }
}

/// Append markdown to an existing note and refresh its index row.
fn append_note(conn: &Connection, notes_dir: &Path, rel: &str, content: &str) -> Result<()> {
    let mut body = arc_index::read_note(notes_dir, rel)?
        .ok_or_else(|| anyhow!("no note at '{rel}'"))?;
    if !body.ends_with('\n') {
        body.push('\n');
    }
    body.push('\n');
    body.push_str(content.trim_end());
    body.push('\n');
    std::fs::write(notes_dir.join(rel), &body)?;
    arc_index::reindex_note(conn, Path::new(rel), &body)?;
    Ok(())
}

/// Create a general note file + index row; returns its `(id, path)`.
fn create_note(
    conn: &Connection,
    notes_dir: &Path,
    title: &str,
    body: &str,
    tags: Vec<String>,
) -> Result<(String, String)> {
    let slug = slugify!(title);
    if slug.is_empty() {
        bail!("title produced an empty slug");
    }
    let rel = format!("{slug}.md");
    let abs = notes_dir.join(&rel);
    if abs.exists() {
        bail!("note '{slug}' already exists; use append_note");
    }
    let mut content = Frontmatter::general(slug.clone(), tags).to_block()?;
    content.push_str(&format!("\n# {title}\n\n{body}\n"));
    if let Some(p) = abs.parent() {
        std::fs::create_dir_all(p)?;
    }
    std::fs::write(&abs, &content)?;
    arc_index::reindex_note(conn, Path::new(&rel), &content)?;
    Ok((slug, rel))
}

/// Create a story-linked note under `stories/` + index row; returns its
/// `(id, path)`. Uses `stories/<slug>.md`, or `stories/<slug>-<story id>.md`
/// when another note already holds that slug.
fn create_story_note(
    conn: &Connection,
    notes_dir: &Path,
    story: &Story,
    iteration_url: Option<String>,
    epic_url: Option<String>,
    body: &str,
) -> Result<(String, String)> {
    let existing = match arc_index::note_for_story(conn, story.id)? {
        Some(rel) => Some(rel),
        None => scan_story_note(notes_dir, story.id)?.map(|(rel, _)| rel),
    };
    if let Some(existing) = existing {
        bail!("story {} already has a note at '{existing}'; use append_note", story.id);
    }
    let base = slugify!(&story.name);
    if base.is_empty() {
        bail!("story name produced an empty slug");
    }
    // existing files here can't link this story; the scan above would've found them
    let Some((slug, rel)) = [base.clone(), format!("{base}-{}", story.id)]
        .into_iter()
        .map(|slug| {
            let rel = format!("stories/{slug}.md");
            (slug, rel)
        })
        .find(|(_, rel)| !notes_dir.join(rel).exists())
    else {
        bail!("notes 'stories/{base}.md' and 'stories/{base}-{}.md' already exist", story.id);
    };
    let abs = notes_dir.join(&rel);
    let mut fm = Frontmatter::story(
        slug.clone(),
        story.id,
        story.name.clone(),
        story.app_url.clone(),
        iteration_url,
    );
    if let EntityLink::Story { epic_url: e, .. } = &mut fm.link {
        *e = epic_url;
    }
    let mut content = fm.to_block()?;
    content.push_str(&format!("\n# {}\n\n{}\n", story.name, body));
    if let Some(p) = abs.parent() {
        std::fs::create_dir_all(p)?;
    }
    std::fs::write(&abs, &content)?;
    arc_index::reindex_note(conn, Path::new(&rel), &content)?;
    Ok((slug, rel))
}

#[tool_handler]
impl ServerHandler for ArcMcp {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::new(ServerCapabilities::builder().enable_tools().build());
        info.server_info = Implementation::new("arc", env!("CARGO_PKG_VERSION"));
        info.instructions = Some(
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
             comments) aren't handled here — use the Shortcut MCP."
                .into(),
        );
        info
    }
}

/// Serve the MCP protocol over stdio until the client disconnects.
pub async fn run_stdio(config: &Config) -> Result<()> {
    // Ensure the index exists (empty is fine) so read-only opens don't fail
    // before the daemon's first run.
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

    fn story(id: i32, name: &str) -> Story {
        Story {
            id,
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
        assert_eq!(created, ("my-note".into(), "my-note.md".into()));
        assert!(create_note(&conn, notes.path(), "My Note", "x", vec![]).is_err());

        let rel = resolve_note(&conn, Some("my-note"), None).unwrap();
        assert_eq!(rel, "my-note.md");
        append_note(&conn, notes.path(), &rel, "more\n\n").unwrap();
        let body = std::fs::read_to_string(notes.path().join(&rel)).unwrap();
        assert!(body.ends_with("body\n\nmore\n"));
        assert!(append_note(&conn, notes.path(), "missing.md", "x").is_err());
    }

    #[test]
    fn create_story_note_links_and_rejects_duplicates() {
        let (_c, notes, conn) = setup();
        let s = story(42, "Optical Flow");
        let created = create_story_note(
            &conn,
            notes.path(),
            &s,
            Some("https://it".into()),
            Some("https://epic".into()),
            "hi",
        )
        .unwrap();
        assert_eq!(created, ("optical-flow".into(), "stories/optical-flow.md".into()));
        assert_eq!(
            arc_index::note_for_story(&conn, 42).unwrap(),
            Some("stories/optical-flow.md".into())
        );
        let body = std::fs::read_to_string(notes.path().join("stories/optical-flow.md")).unwrap();
        let fm = Frontmatter::from_note(&body).unwrap();
        assert!(matches!(
            fm.link,
            EntityLink::Story { id: 42, epic_url: Some(ref e), iteration_url: Some(_), .. } if e == "https://epic"
        ));
        let err = create_story_note(&conn, notes.path(), &s, None, None, "again").unwrap_err();
        assert!(err.to_string().contains("stories/optical-flow.md"), "{err}");
    }

    #[test]
    fn create_story_note_sidesteps_another_storys_slug() {
        let (_c, notes, conn) = setup();
        create_story_note(&conn, notes.path(), &story(1, "Same Name"), None, None, "a").unwrap();
        let created =
            create_story_note(&conn, notes.path(), &story(2, "Same Name"), None, None, "b").unwrap();
        assert_eq!(created, ("same-name-2".into(), "stories/same-name-2.md".into()));
        assert_eq!(
            arc_index::note_for_story(&conn, 2).unwrap(),
            Some("stories/same-name-2.md".into())
        );
        let (rel, body) = story_note(&conn, notes.path(), 1).unwrap().unwrap();
        assert_eq!(rel, "stories/same-name.md");
        assert!(body.ends_with("a\n"));

        // a plain (unlinked) note on the slug is sidestepped too
        std::fs::write(notes.path().join("stories/other.md"), "---\nid: other\n---\n").unwrap();
        let created = create_story_note(&conn, notes.path(), &story(3, "Other"), None, None, "c").unwrap();
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
        let err = create_story_note(&conn, notes.path(), &story(5, "Five"), None, None, "x")
            .unwrap_err()
            .to_string();
        assert!(err.contains("stories/renamed.md"), "{err}");
        assert!(!notes.path().join("stories/five.md").exists());
    }

    #[test]
    fn rel_path_validation() {
        assert_eq!(check_rel_path("stories/a.md").unwrap(), "stories/a.md");
        assert_eq!(check_rel_path("./a.md").unwrap(), "a.md");
        assert_eq!(check_rel_path(".//stories//./a.md").unwrap(), "stories/a.md");
        for bad in ["../a.md", "stories/../../a.md", "/etc/a.md", "", ".", "a.txt", "stories"] {
            assert!(check_rel_path(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn append_by_unnormalised_path_keeps_one_index_row() {
        let (_c, notes, conn) = setup();
        create_story_note(&conn, notes.path(), &story(8, "X"), None, None, "b").unwrap();
        let rel = resolve_note(&conn, None, Some("./stories//x.md")).unwrap();
        assert_eq!(rel, "stories/x.md");
        append_note(&conn, notes.path(), &rel, "more").unwrap();
        let rows = arc_index::query(&conn, "SELECT path FROM notes").unwrap();
        assert_eq!(rows, r#"[{"path":"stories/x.md"}]"#);
    }

    #[test]
    fn resolve_note_by_id_and_path() {
        let (_c, _n, conn) = setup();
        arc_index::reindex_note(&conn, Path::new("foo.md"), "---\nid: foo\n---\n").unwrap();
        arc_index::reindex_note(&conn, Path::new("stories/foo.md"), "---\nid: foo\n---\n")
            .unwrap();
        let err = resolve_note(&conn, Some("foo"), None).unwrap_err().to_string();
        assert!(err.contains("foo.md") && err.contains("stories/foo.md"), "{err}");
        assert_eq!(
            resolve_note(&conn, Some("foo"), Some("stories/foo.md")).unwrap(),
            "stories/foo.md"
        );
        assert!(resolve_note(&conn, None, Some("../x.md")).is_err());
        assert!(resolve_note(&conn, Some("nope"), None).is_err());
        assert!(resolve_note(&conn, None, None).is_err());
    }

    #[test]
    fn story_note_scans_for_unindexed_notes() {
        let (_c, notes, conn) = setup();
        assert_eq!(story_note(&conn, notes.path(), 7).unwrap(), None);

        // same slug, but not linked to this story
        std::fs::create_dir_all(notes.path().join("stories")).unwrap();
        std::fs::write(notes.path().join("stories/some-story.md"), "unindexed").unwrap();
        std::fs::write(
            notes.path().join("stories/a-other.md"),
            "---\nid: a\nstory_id: sc-70\n---\n",
        )
        .unwrap();
        assert_eq!(story_note(&conn, notes.path(), 7).unwrap(), None);

        let linked = "---\nid: some-story-7\nstory_id: sc-7\n---\nunindexed";
        std::fs::write(notes.path().join("stories/some-story-7.md"), linked).unwrap();
        assert_eq!(
            story_note(&conn, notes.path(), 7).unwrap(),
            Some(("stories/some-story-7.md".into(), linked.into()))
        );

        // index wins over the scan
        std::fs::write(notes.path().join("other.md"), "indexed").unwrap();
        arc_index::reindex_note(&conn, Path::new("other.md"), "---\nid: o\nstory_id: sc-7\n---\n")
            .unwrap();
        assert_eq!(
            story_note(&conn, notes.path(), 7).unwrap(),
            Some(("other.md".into(), "indexed".into()))
        );
    }

    #[test]
    fn slim_stories_shape() {
        let (_c, _n, conn) = setup();
        arc_index::reindex_note(&conn, Path::new("stories/a.md"), "---\nid: a\nstory_id: sc-1\n---\n")
            .unwrap();
        let mut a = story(1, "A");
        a.workflow_state_id = Some(500);
        a.story_type = "feature".into();
        a.estimate = Some(3);
        a.description = "long".into();
        let mut cache = Cache {
            iteration_stories: Some(vec![a, story(2, "B")]),
            ..Default::default()
        };
        cache.workflow_states.insert(500, "In Progress".into());

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
        let owner = Uuid::from_u128(1);
        let unknown = Uuid::from_u128(2);
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
    fn store_story_in_place_or_by_id() {
        let mut cache = Cache {
            iteration_stories: Some(vec![story(1, "old")]),
            ..Default::default()
        };
        store_story(&mut cache, story(1, "new"));
        assert_eq!(cache.find_story(1).unwrap().name, "new");
        assert!(cache.stories_by_id.is_empty());
        store_story(&mut cache, story(9, "outside"));
        assert_eq!(cache.stories_by_id[&9].name, "outside");
    }

    #[test]
    fn store_story_refreshes_active_and_by_id_copies() {
        let mut cache = Cache {
            active_story: Some(story(5, "old")),
            stories_by_id: [(5, story(5, "old"))].into(),
            ..Default::default()
        };
        store_story(&mut cache, story(5, "new"));
        assert_eq!(cache.find_story(5).unwrap().name, "new");
        assert_eq!(cache.active_story.as_ref().unwrap().name, "new");

        // active story alone gets an up-to-date stories_by_id entry too
        let mut cache = Cache {
            active_story: Some(story(6, "old")),
            ..Default::default()
        };
        store_story(&mut cache, story(6, "new"));
        assert_eq!(cache.stories_by_id[&6].name, "new");
        assert_eq!(cache.active_story.unwrap().name, "new");
    }
}
