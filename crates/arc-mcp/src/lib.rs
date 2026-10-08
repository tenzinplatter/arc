use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
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

use arc_context::{Checkout, EntryText, SessionRequest, GitHead, Handoff, LoggedSession, NoteLinks, SectionHeading, StoryNote, Vault};
use arc_core::{Config, StoryId};
use arc_index::Connection;
use arc_notes::note::frontmatter::Frontmatter;
use arc_notes::{BranchName, NoteRelPath, SessionId, TodoScope};
use arc_shortcut::api::branch::Branch;
use arc_shortcut::api::story::Story;
use arc_shortcut::{ApiClient, Cache};

pub mod http;

pub use http::{Auth, HttpBinding, MCP_PATH, Transport};

const QUERY_ROW_CAP: usize = 200;
const NET_TIMEOUT: Duration = Duration::from_secs(10);
const LOOKUP_REFRESH_INTERVAL: Duration = Duration::from_secs(10 * 60);

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
    /// Optional `## ` section to append under, by its heading text without the `#`s (e.g. `Log`). The content goes at the end of that section, which is created at the end of the note if missing. Omit to append at the end of the note.
    #[serde(default)]
    section: Option<String>,
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

#[derive(Deserialize)]
#[serde(untagged)]
enum StoryIdInput {
    Number(i64),
    Text(String),
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(try_from = "StoryIdInput")]
struct StoryRef(StoryId);

impl TryFrom<StoryIdInput> for StoryRef {
    type Error = anyhow::Error;

    fn try_from(input: StoryIdInput) -> Result<Self> {
        match input {
            StoryIdInput::Number(n) => n.to_string().parse(),
            StoryIdInput::Text(s) => s.trim().parse(),
        }
        .map(Self)
    }
}

#[derive(Deserialize, JsonSchema)]
struct LogSessionArgs {
    /// Shortcut story id, as a number (`123`) or a string (`"sc-123"`). Defaults to the story resolved from `branch`, or from the git branch checked out in `cwd` (an `sc-<id>` in the branch name, or a story note listing the branch under `branches:`). On a remote (HTTP) arc server, `story_id` alone is enough when you have no branch.
    #[serde(default)]
    #[schemars(schema_with = "optional_story_id_schema")]
    story_id: Option<StoryRef>,
    /// Absolute path of your current working directory: the git checkout or worktree you are working in. The story and branch are resolved from it. Defaults to the directory the arc MCP server was started in, which may be a different checkout or worktree. Rejected by a remote (HTTP) arc server, which can't see your filesystem; pass `branch` there instead.
    #[serde(default)]
    cwd: Option<String>,
    /// Name of the git branch you are working on (e.g. `me/sc-123/thing`). The story resolves from the branch name alone, without reading any checkout: an `sc-<id>` in the name, or a story note listing the branch under `branches:`. Pass this instead of `cwd` when the arc server runs on another machine (HTTP transport). Don't pass both.
    #[serde(default)]
    branch: Option<String>,
    /// What this session got done.
    done: String,
    /// Where things stand now: what works, what's half-done, anything uncommitted or broken.
    state: String,
    /// The next concrete steps for whoever picks this up.
    next: String,
    /// Unresolved questions or decisions, if any.
    #[serde(default)]
    questions: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct CurrentStoryArgs {
    /// Absolute path of your current working directory: the git checkout or worktree you are working in. The story and branch are resolved from it. Defaults to the directory the arc MCP server was started in, which may be a different checkout or worktree. Rejected by a remote (HTTP) arc server, which can't see your filesystem; pass `branch` there instead.
    #[serde(default)]
    cwd: Option<String>,
    /// Name of the git branch you are working on (e.g. `me/sc-123/thing`). The story resolves from the branch name alone, without reading any checkout: an `sc-<id>` in the name, or a story note listing the branch under `branches:`. Pass this instead of `cwd` when the arc server runs on another machine (HTTP transport). Don't pass both.
    #[serde(default)]
    branch: Option<String>,
}

const REMOTE_NEEDS_BRANCH: &str = "this arc server runs on another machine and can't see your checkout; pass your git branch name as `branch`";

#[derive(Clone, Debug)]
struct ServerCwd(PathBuf);

impl ServerCwd {
    fn checkout(&self) -> Checkout {
        Checkout::Dir(self.0.clone())
    }

    fn work_dir(&self, dir: &str) -> Result<PathBuf> {
        let path = PathBuf::from(dir);
        if !path.is_absolute() {
            bail!("`cwd` must be an absolute path, got '{dir}'");
        }
        if !path.is_dir() {
            bail!("`cwd` '{dir}' is not an existing directory");
        }
        Ok(path)
    }
}

#[derive(Clone, Debug)]
enum Locality {
    Local(ServerCwd),
    Remote,
}

impl Locality {
    fn checkout(&self, cwd: Option<&str>, branch: Option<&str>) -> Result<Option<Checkout>> {
        match (self, cwd, branch) {
            (_, Some(_), Some(_)) => bail!("pass either `cwd` or `branch`, not both"),
            (Locality::Remote, Some(_), None) => {
                bail!("this server can't see your filesystem; pass `branch`")
            }
            (Locality::Local(server), Some(dir), None) => server.work_dir(dir).map(|dir| Some(Checkout::Dir(dir))),
            (_, None, Some(branch)) => BranchName::parse(branch.trim())
                .map(|b| Some(Checkout::Branch(b)))
                .map_err(|e| anyhow!("`branch`: {e}")),
            (Locality::Local(server), None, None) => Ok(Some(server.checkout())),
            (Locality::Remote, None, None) => Ok(None),
        }
    }

    fn todo_dir(&self, cwd: Option<&str>) -> Result<PathBuf> {
        match (self, cwd) {
            (Locality::Remote, _) => bail!("this server can't see your filesystem; pass `global: true`"),
            (Locality::Local(server), Some(dir)) => server.work_dir(dir),
            (Locality::Local(ServerCwd(dir)), None) => Ok(dir.clone()),
        }
    }
}

fn session_request(checkout: Option<Checkout>, story: Option<StoryId>) -> Result<SessionRequest> {
    match (checkout, story) {
        (Some(checkout), None) => Ok(SessionRequest::Resolve(checkout)),
        (checkout, Some(id)) => Ok(SessionRequest::Story { id, checkout }),
        (None, None) => bail!("{REMOTE_NEEDS_BRANCH}, or the story as `story_id`"),
    }
}

fn optional_story_id_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({ "type": ["integer", "string", "null"] })
}

fn entry_field(name: &str, text: &str) -> Result<EntryText> {
    EntryText::parse(text).map_err(|e| anyhow!("`{name}`: {e}"))
}

impl LogSessionArgs {
    fn into_parts(self, locality: &Locality) -> Result<(SessionRequest, Handoff)> {
        let checkout = locality.checkout(self.cwd.as_deref(), self.branch.as_deref())?;
        let request = session_request(checkout, self.story_id.map(|StoryRef(id)| id))?;
        let handoff = Handoff {
            done: entry_field("done", &self.done)?,
            state: entry_field("state", &self.state)?,
            next: entry_field("next", &self.next)?,
            questions: self
                .questions
                .as_deref()
                .filter(|q| !q.trim().is_empty())
                .map(|q| entry_field("questions", q))
                .transpose()?,
        };
        Ok((request, handoff))
    }
}

#[derive(Deserialize, JsonSchema)]
struct AddTodoArgs {
    /// Todo text.
    text: String,
    /// Optional due date, `YYYY-MM-DD`.
    #[serde(default)]
    date: Option<String>,
    /// Absolute path of your current working directory. The todo is scoped to the repo named by the `origin` remote there. Defaults to the directory the arc MCP server was started in.
    #[serde(default)]
    cwd: Option<String>,
    /// Your Claude Code session id, so the todo shows up as this session's.
    #[serde(default)]
    session_id: Option<String>,
    /// True for a todo that belongs to no repo; it goes to `todos.md`.
    #[serde(default)]
    global: bool,
}

#[derive(Clone)]
pub struct ArcMcp {
    config: Config,
    locality: Locality,
    client: Arc<OnceCell<ApiClient>>,
    last_lookup_refresh: Arc<Mutex<Option<Instant>>>,
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
    pub fn new(config: Config, cwd: PathBuf) -> Self {
        Self::with_locality(config, Locality::Local(ServerCwd(cwd)))
    }

    pub fn remote(config: Config) -> Self {
        Self::with_locality(config, Locality::Remote)
    }

    fn with_locality(config: Config, locality: Locality) -> Self {
        Self {
            config,
            locality,
            client: Arc::new(OnceCell::new()),
            last_lookup_refresh: Arc::new(Mutex::new(None)),
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
        let cache_dir = self.config.cache_dir.clone();
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
        let cache_dir = self.config.cache_dir.clone();
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
        let (cache_dir, notes_dir) = (self.config.cache_dir.clone(), self.config.notes_dir.clone());
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
        description = "Run a read-only SQL query against the notes index and return rows as a JSON array of objects (at most 200 rows). Tables: notes(path, id, title, created, type, link_kind['story'|'iteration'|'epic'|null], entity_id, entity_url, entity_name, iteration_url, epic_url); note_tags(path, tag); note_branches(path, branch); todos(id, text, date, completed, file, line, position, repo['<host>/<owner>/<repo>'|null], session). Only SELECT/WITH/EXPLAIN/PRAGMA are permitted."
    )]
    async fn query(
        &self,
        Parameters(QueryArgs { sql }): Parameters<QueryArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let cache_dir = self.config.cache_dir.clone();
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
        let (cache_dir, notes_dir) = (self.config.cache_dir.clone(), self.config.notes_dir.clone());
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
        description = "Append Markdown to an existing note (by notes-dir-relative `path`, or Obsidian `id`). Use this to persist context or progress into a note. Non-destructive — content is added below what's already there, or at the end of the `## ` section named by `section` (created if missing). For session handoffs on the current story use `log_session`."
    )]
    async fn append_note(
        &self,
        Parameters(AppendNoteArgs { id, path, content, section }): Parameters<AppendNoteArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let (cache_dir, notes_dir) = (self.config.cache_dir.clone(), self.config.notes_dir.clone());
        let appended = tokio::task::spawn_blocking(move || -> Result<String> {
            let section = section.as_deref().map(SectionHeading::parse).transpose()?;
            let conn = arc_index::open(&cache_dir)?;
            let vault = Vault::new(&conn, &notes_dir, &cache_dir);
            let rel = resolve_note(&conn, NoteRef::from_args(id, path)?)?;
            Ok(match section {
                Some(heading) => {
                    append_note_section(&vault, &rel, &content, &heading)?;
                    format!("appended to {rel} under {heading}")
                }
                None => {
                    append_note(&vault, &rel, &content)?;
                    format!("appended to {rel}")
                }
            })
        })
        .await
        .map_err(internal)?;
        Ok(CallToolResult::success(vec![ContentBlock::text(tool_try!(appended))]))
    }

    #[tool(
        description = "Create a new general note (no Shortcut link). Returns the new note's `id` and notes-dir-relative `path`; pass that `path` to `append_note` to add to it later. Errors if a note with the same slug already exists — append_note to add to it instead."
    )]
    async fn create_note(
        &self,
        Parameters(CreateNoteArgs { title, body, tags }): Parameters<CreateNoteArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let (cache_dir, notes_dir) = (self.config.cache_dir.clone(), self.config.notes_dir.clone());
        let created = tokio::task::spawn_blocking(move || {
            let conn = arc_index::open(&cache_dir)?;
            create_note(&Vault::new(&conn, &notes_dir, &cache_dir), &title, &body, tags)
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

        let (cache_dir, notes_dir) = (self.config.cache_dir.clone(), self.config.notes_dir.clone());
        let created = tokio::task::spawn_blocking(move || {
            let conn = arc_index::open(&cache_dir)?;
            arc_context::create_story_note(&Vault::new(&conn, &notes_dir, &cache_dir), &story, links, &body)
        })
        .await
        .map_err(internal)?;
        Ok(story_note_result(story_id, tool_try!(created)))
    }

    #[tool(
        description = "Record a session handoff on a Shortcut story: appends a `### <local time> <UTC offset>` entry (followed by ` · <branch>` when the branch belongs to the story) with Done / State / Next / Open questions to the end of the story note's `## Sessions` section, creating the section (and the story note) if missing. Use it at milestones and always before ending a session — it captures where things stand so the next session can pick up from it. Pass your current working directory as `cwd` (the server's own directory may be another checkout or worktree); the story defaults to the one resolved from the git branch checked out there. When the arc server runs on another machine (HTTP transport) it can't see your filesystem and rejects `cwd`: pass your git branch name as `branch`, or just `story_id` when you have no branch (the entry then names no branch). Pass `story_id` for any other story; the entry names the branch only when it belongs to that story. Returns the note path and the entry heading, plus the directory and/or git branch it resolved against when one was used."
    )]
    async fn log_session(
        &self,
        Parameters(args): Parameters<LogSessionArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let (request, handoff) = tool_try!(args.into_parts(&self.locality));
        let mut cache = self.read_cache().await;
        let logged = tool_try!(
            arc_context::log_session_at(&request, &self.config, &mut cache, handoff)
                .await
                .map_err(|e| format!("{e:#}"))
        );
        Ok(logged_result(&request, &logged))
    }

    #[tool(
        description = "The Shortcut story for the git branch checked out in `cwd` (or named by `branch`), as JSON: story id, name, state, type and url; the branch and how it resolved to the story; the canonical branch name; the story note path; the latest `## Sessions` handoff entries; and the note's open todos. Same payload as `arc context --format json` — use it when the context shown at session start has scrolled away. Pass your current working directory as `cwd` (the server's own directory may be another checkout or worktree). When the arc server runs on another machine (HTTP transport) it can't see your filesystem and rejects `cwd`: pass your git branch name as `branch` instead. A second text block names the branch it resolved against, and the directory when `cwd` was used. Returns a plain message, not an error, when no story resolves for the branch."
    )]
    async fn current_story(
        &self,
        Parameters(CurrentStoryArgs { cwd, branch }): Parameters<CurrentStoryArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let checkout = tool_try!(
            self.locality
                .checkout(cwd.as_deref(), branch.as_deref())
                .and_then(|checkout| checkout.ok_or_else(|| anyhow!(REMOTE_NEEDS_BRANCH)))
        );
        let mut cache = self.read_cache().await;
        let context = tool_try!(
            arc_context::current_context_at(&checkout, &self.config, &mut cache)
                .await
                .map_err(|e| format!("{e:#}"))
        );
        let Some(context) = context else {
            let head_checkout = checkout.clone();
            let head = tokio::task::spawn_blocking(move || head_checkout.head())
                .await
                .map_err(internal)?;
            let head = tool_try!(head.map_err(|e| format!("{e:#}")));
            return Ok(CallToolResult::success(vec![ContentBlock::text(format!(
                "no story for this branch\n{}",
                resolved_against(&checkout, &branch_label(&head))
            ))]));
        };
        let json = tool_try!(serde_json::to_string_pretty(&context));
        Ok(with_notes(
            json,
            [resolved_against(&checkout, context.branch.as_str())],
        ))
    }

    #[tool(
        description = "Add a todo as a checkbox in the notes dir: in the repo's todo note (`todos/<host>/<owner>/<repo>.md`, keyed by the `origin` remote of the git repo at `cwd`), or in `todos.md` when `global` is true or `cwd` has no `origin` remote on a host. Shows up in the arc TUI and is indexed for querying. Optional due date as YYYY-MM-DD. Returns the new todo's id and the note it went to."
    )]
    async fn add_todo(
        &self,
        Parameters(AddTodoArgs { text, date, cwd, session_id, global }): Parameters<AddTodoArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let date = match date.as_deref() {
            Some(d) => Some(tool_try!(
                NaiveDate::parse_from_str(d, "%Y-%m-%d").map_err(|e| format!("bad date: {e}"))
            )),
            None => None,
        };
        let session = tool_try!(session_id.as_deref().map(SessionId::parse).transpose().map_err(|e| format!("{e:#}")));
        let repo = match global {
            true => None,
            false => {
                let dir = tool_try!(self.locality.todo_dir(cwd.as_deref()));
                let found = tokio::task::spawn_blocking(move || arc_context::git::repo_key(&dir))
                    .await
                    .map_err(internal)?;
                tool_try!(found.map_err(|e| format!("{e:#}")))
            }
        };
        let scope = TodoScope { repo, session };
        let todo = tool_try!(arc_context::add_todo(&self.config.notes_dir, &self.config.cache_dir, text, date, scope).await);
        Ok(CallToolResult::success(vec![ContentBlock::text(format!("added {} to {}", todo.id, todo.file))]))
    }
}

impl ArcMcp {
    fn claim_lookup_refresh(&self, now: Instant) -> bool {
        let mut last = self
            .last_lookup_refresh
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let due = last.is_none_or(|at| now.duration_since(at) >= LOOKUP_REFRESH_INTERVAL);
        if due {
            *last = Some(now);
        }
        due
    }

    async fn read_cache(&self) -> Cache {
        // a missing cache file defaults to ~/.cache/arc; writes must go to ours
        Cache::read(self.config.cache_dir.clone())
            .await
            .with_cache_dir(self.config.cache_dir.clone())
    }

    async fn client(&self, cache: &mut Cache) -> Result<ApiClient> {
        self.client
            .get_or_try_init(|| timed(ApiClient::from_token_and_cache(&self.config.api_token, cache)))
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
            if missing(cache) && self.claim_lookup_refresh(Instant::now()) {
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

fn story_note_result(story_id: StoryId, note: StoryNote) -> CallToolResult {
    match note {
        StoryNote::Created { slug, rel } => created_result((slug, rel)),
        StoryNote::Existing(rel) => fail(format!(
            "story {story_id} already has a note at '{rel}'; use append_note"
        )),
    }
}

fn branch_label(head: &GitHead) -> String {
    match head {
        GitHead::Branch(branch) => branch.to_string(),
        GitHead::Detached => "(detached HEAD)".into(),
        GitHead::NotARepo => "(not a git repository)".into(),
    }
}

fn resolved_against(checkout: &Checkout, branch: &str) -> String {
    match checkout {
        Checkout::Dir(dir) => format!("cwd: {}\nbranch: {branch}", dir.display()),
        Checkout::Branch(_) => format!("branch: {branch}"),
    }
}

fn logged_result(request: &SessionRequest, logged: &LoggedSession) -> CallToolResult {
    let checkout = match request {
        SessionRequest::Resolve(checkout) => Some(checkout),
        SessionRequest::Story { checkout, .. } => checkout.as_ref(),
    };
    let summary = std::iter::once(format!("path: {}\nentry: {}", logged.note.rel(), logged.entry.header()))
        .chain(
            checkout
                .zip(logged.head.as_ref())
                .map(|(checkout, head)| resolved_against(checkout, &branch_label(head))),
        )
        .collect::<Vec<_>>()
        .join("\n");
    let created = match &logged.note {
        StoryNote::Created { rel, .. } => Some(format!("note: created {rel} for story {}", logged.story)),
        StoryNote::Existing(_) => None,
    };
    with_notes(summary, created)
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

fn append_note(vault: &Vault, rel: &NoteRelPath, content: &str) -> Result<()> {
    vault.edit(rel, |existing| Ok(arc_context::append_to_end(existing, content)))?;
    Ok(())
}

fn append_note_section(vault: &Vault, rel: &NoteRelPath, content: &str, heading: &SectionHeading) -> Result<()> {
    vault.edit(rel, |existing| Ok(arc_context::append_to_section(existing, heading, content)))?;
    Ok(())
}

fn create_note(vault: &Vault, title: &str, body: &str, tags: Vec<String>) -> Result<(String, NoteRelPath)> {
    let slug = slugify!(title);
    if slug.is_empty() {
        bail!("title produced an empty slug");
    }
    let rel = NoteRelPath::parse(&format!("{slug}.md"))?;
    if rel.abs(vault.dir()).exists() {
        bail!("note '{slug}' already exists; use append_note");
    }
    let content = format!(
        "{}\n# {title}\n\n{body}\n",
        Frontmatter::general(slug.clone(), tags).to_block()?
    );
    vault
        .create(&rel, &content)
        .with_context(|| format!("couldn't create note '{slug}'; if it exists, use append_note"))?;
    Ok((slug, rel))
}

#[tool_handler]
impl ServerHandler for ArcMcp {
    fn get_info(&self) -> ServerInfo {
        let checkout = match self.locality {
            Locality::Local(_) => {
                "Always pass your current working directory as `cwd` to both: this server's own \
                 directory is fixed at startup and may be another checkout or worktree. Both echo \
                 the directory and branch they resolved against"
            }
            Locality::Remote => {
                "This server runs on another machine and can't see your filesystem: pass your \
                 current git branch name as `branch` to both (never `cwd`, which is rejected). \
                 `log_session` also accepts just `story_id` when you have no branch, e.g. from a \
                 phone. Both echo the branch they resolved against"
            }
        };
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("arc", env!("CARGO_PKG_VERSION")))
            .with_instructions(format!(
            "arc: the notes vault plus a cache of Shortcut stories. Stories: \
             `active_stories` (slim list of the current iterations), `get_story` (full \
             detail incl. description and comments), `get_story_note` (a story's note). \
             Notes: `query` (read-only SQL over notes, tags, branches, todos), `get_note` \
             (full Markdown by path or id), `append_note` to add context to a note (optionally \
             under a `## ` `section`), `create_note` for a new note, `create_story_note` for a \
             note linked to a story, `add_todo` for a todo. Current work: `current_story` \
             returns the story for your git branch (story, note, recent \
             `## Sessions` handoffs, open todos); `log_session` records a handoff (done / state / \
             next / open questions) in that story's note — call it at milestones and before \
             ending a session. {checkout}. `active_stories` lists what the arc TUI last \
             cached for the current iteration(s) and may be stale (use the Shortcut MCP \
             for live search); `get_story` refetches the story from Shortcut, falling back \
             to the cache when it's unreachable. Shortcut writes (creating stories, state changes, \
             comments) aren't handled here — use the Shortcut MCP.",
            ))
    }
}

pub async fn run(config: &Config, transport: Transport) -> Result<()> {
    match transport {
        Transport::Stdio => run_stdio(config).await,
        Transport::Http(binding) => http::run(config, binding).await,
    }
}

pub async fn run_stdio(config: &Config) -> Result<()> {
    // created up front so read-only opens work before the daemon's first run
    arc_index::open(&config.cache_dir)?;

    let server = ArcMcp::new(config.clone(), std::env::current_dir()?);
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use arc_shortcut::api::story::comment::StoryComment;
    use arc_shortcut::api::user::MemberId;
    use uuid::Uuid;

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

    fn vault<'a>(conn: &'a Connection, notes: &'a tempfile::TempDir, cache: &tempfile::TempDir) -> Vault<'a> {
        Vault::new(conn, notes.path(), cache.path())
    }

    #[test]
    fn create_and_append_note() {
        let (cache, notes, conn) = setup();
        let v = vault(&conn, &notes, &cache);
        let created = create_note(&v, "My Note", "body", vec!["t".into()]).unwrap();
        assert_eq!(created, ("my-note".into(), rel("my-note.md")));
        assert!(create_note(&v, "My Note", "x", vec![]).is_err());

        let rel = resolve(&conn, Some("my-note"), None).unwrap();
        assert_eq!(rel, "my-note.md");
        append_note(&v, &rel, "more\n\n").unwrap();
        let body = std::fs::read_to_string(notes.path().join(&rel)).unwrap();
        assert!(body.ends_with("body\n\nmore\n"));
        assert!(append_note(&v, &NoteRelPath::parse("missing.md").unwrap(), "x").is_err());
    }

    #[test]
    fn append_by_unnormalised_path_keeps_one_index_row() {
        let (cache, notes, conn) = setup();
        let v = vault(&conn, &notes, &cache);
        arc_context::create_story_note(&v, &story(8, "X"), NoteLinks::default(), "b").unwrap();
        let rel = resolve(&conn, None, Some("./stories//x.md")).unwrap();
        assert_eq!(rel, "stories/x.md");
        append_note(&v, &rel, "more").unwrap();
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
    fn story_note_result_points_existing_notes_at_append_note() {
        let (cache, notes, conn) = setup();
        let v = vault(&conn, &notes, &cache);
        let created = arc_context::create_story_note(&v, &story(8, "X"), NoteLinks::default(), "").unwrap();
        assert_eq!(
            serde_json::to_value(story_note_result(8.into(), created)).unwrap()["content"][0]["text"],
            "id: x\npath: stories/x.md"
        );
        let body = std::fs::read_to_string(notes.path().join("stories/x.md")).unwrap();
        assert!(body.ends_with("---\n\n# X\n"), "{body:?}");
        let again = arc_context::create_story_note(&v, &story(8, "X"), NoteLinks::default(), "y").unwrap();
        assert_eq!(
            serde_json::to_value(story_note_result(8.into(), again)).unwrap(),
            json!({
                "content": [{"type": "text", "text": "story 8 already has a note at 'stories/x.md'; use append_note"}],
                "isError": true
            })
        );
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
        let (cache, notes, conn) = setup();
        let v = vault(&conn, &notes, &cache);
        let (mut cache, s) = golden_detail_story();
        arc_context::create_story_note(&v, &s, NoteLinks::default(), "hi").unwrap();
        cache.iteration_stories = Some(vec![s, story(8, "eight")]);
        assert_eq!(
            slim_stories(&cache, &conn).unwrap(),
            json!([
                {"id": 7, "name": "story seven", "state": "In Progress", "type": "feature", "url": "https://app.shortcut.com/x/story/7", "iteration_id": 3, "epic_id": 40, "estimate": 3, "has_note": true},
                {"id": 8, "name": "eight", "state": null, "type": "", "url": "https://app.shortcut.com/x/story/8", "iteration_id": null, "epic_id": null, "estimate": null, "has_note": false}
            ])
        );
    }

    fn config_in(root: &Path) -> Config {
        Config {
            notes_dir: root.join("notes"),
            cache_dir: root.join("cache"),
            api_token: String::new(),
            editor: String::new(),
            repositories_directory: root.to_path_buf(),
            mux: Default::default(),
            notifier: Default::default(),
            mcp_token: None,
        }
    }

    fn git_repo_on(root: &Path, branch: &str) -> Option<PathBuf> {
        let git = |dir: &Path, args: &[&str]| {
            std::process::Command::new("git").arg("-C").arg(dir).args(args).output()
        };
        if git(root, &["--version"]).is_err() {
            eprintln!("git not found; skipping");
            return None;
        }
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        for args in [vec!["init", "-q"], vec!["symbolic-ref", "HEAD", &format!("refs/heads/{branch}")]] {
            assert!(git(&repo, &args).unwrap().status.success(), "git {args:?}");
        }
        Some(repo)
    }

    async fn server_with_story(root: &Path, cwd: PathBuf) -> ArcMcp {
        let config = config_in(root);
        let mut cache = Cache::default().with_cache_dir(config.cache_dir.clone());
        cache.upsert_story(story(7, "Story seven"));
        cache.write().await.unwrap();
        ArcMcp::new(config, cwd)
    }

    fn result_texts(result: &CallToolResult) -> (bool, Vec<String>) {
        let v = serde_json::to_value(result).unwrap();
        let texts = v["content"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["text"].as_str().unwrap().to_owned())
            .collect();
        (v["isError"] == true, texts)
    }

    fn log_args(v: Value) -> Parameters<LogSessionArgs> {
        Parameters(serde_json::from_value(v).unwrap())
    }

    #[test]
    fn story_ids_parse_from_numbers_and_strings() {
        let id = |v: Value| serde_json::from_value::<StoryRef>(v).map(|StoryRef(id)| id.get()).ok();
        assert_eq!(id(json!(7)), Some(7));
        assert_eq!(id(json!("sc-7")), Some(7));
        assert_eq!(id(json!(" SC-7 ")), Some(7));
        assert_eq!(id(json!("7")), Some(7));
        assert_eq!(id(json!(0)), None);
        assert_eq!(id(json!(-3)), None);
        assert_eq!(id(json!("x")), None);
        assert_eq!(id(json!(1.5)), None);
    }

    #[test]
    fn log_session_args_reject_empty_fields() {
        let args = |v: Value| serde_json::from_value::<LogSessionArgs>(v).unwrap().into_parts(&Locality::Remote);
        let err = args(json!({"done": " ", "state": "s", "next": "n"})).unwrap_err();
        assert!(err.to_string().contains("`story_id`"), "{err}");
        let err = args(json!({"story_id": 3, "done": " ", "state": "s", "next": "n"})).unwrap_err();
        assert_eq!(err.to_string(), "`done`: empty session field");
        let (request, handoff) = args(json!({"story_id": "sc-3", "done": "d", "state": "s", "next": "n", "questions": "  "})).unwrap();
        assert_eq!(request, SessionRequest::Story { id: 3.into(), checkout: None });
        assert_eq!(handoff.questions, None);
    }

    #[test]
    fn new_tools_are_registered_with_their_schemas() {
        let tools = ArcMcp::tool_router().list_all();
        let tool = |name: &str| tools.iter().find(|t| t.name == name).unwrap();
        let log = serde_json::to_value(&tool("log_session").input_schema).unwrap();
        assert_eq!(log["required"], json!(["done", "state", "next"]));
        assert_eq!(log["properties"]["story_id"]["type"], json!(["integer", "string", "null"]));
        assert!(log["properties"]["cwd"].is_object());
        let current = serde_json::to_value(&tool("current_story").input_schema).unwrap();
        assert_eq!(current["properties"].as_object().unwrap().keys().collect::<Vec<_>>(), ["branch", "cwd"]);
        assert!(log["properties"]["branch"].is_object());
        assert!(current.get("required").is_none_or(|r| r == &json!([])), "{current}");
        let append = serde_json::to_value(&tool("append_note").input_schema).unwrap();
        assert_eq!(append["required"], json!(["content"]));
        assert!(append["properties"]["section"].is_object());
    }

    #[tokio::test]
    async fn log_session_by_explicit_id_creates_the_note() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let mcp = server_with_story(tmp.path(), outside.path().to_path_buf()).await;
        let result = mcp
            .log_session(log_args(json!({"story_id": "sc-7", "done": "a\nb", "state": "s", "next": "n", "questions": "q"})))
            .await
            .unwrap();
        let (is_error, texts) = result_texts(&result);
        assert!(!is_error, "{texts:?}");
        let [summary, created] = texts.as_slice() else { panic!("{texts:?}") };
        assert_eq!(created, "note: created stories/story-seven.md for story 7");
        let [path, header, cwd, branch] = summary.lines().collect::<Vec<_>>()[..] else { panic!("{summary}") };
        assert_eq!(path, "path: stories/story-seven.md");
        let header = header.strip_prefix("entry: ").unwrap();
        assert!(header.starts_with("### ") && !header.contains(" · "), "{header}");
        assert_eq!(cwd, format!("cwd: {}", outside.path().display()));
        assert_eq!(branch, "branch: (not a git repository)");
        let body = std::fs::read_to_string(tmp.path().join("notes/stories/story-seven.md")).unwrap();
        assert!(
            body.ends_with(&format!("# Story seven\n\n## Sessions\n\n{header}\n**Done:** a\n    b\n**State:** s\n**Next:** n\n**Open questions:** q\n")),
            "{body}"
        );
        let conn = arc_index::open_readonly(&tmp.path().join("cache")).unwrap();
        assert_eq!(arc_index::note_for_story(&conn, 7.into()).unwrap(), Some(rel("stories/story-seven.md")));
    }

    #[tokio::test]
    async fn log_session_resolves_the_story_from_the_cwd_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let Some(repo) = git_repo_on(tmp.path(), "me/sc-7/x") else { return };
        let mcp = server_with_story(tmp.path(), repo.clone()).await;
        let conn = arc_index::open(&tmp.path().join("cache")).unwrap();
        let notes = tmp.path().join("notes");
        arc_context::create_story_note(&Vault::new(&conn, &notes, &tmp.path().join("cache")), &story(7, "Story seven"), NoteLinks::default(), "b").unwrap();
        let result = mcp
            .log_session(log_args(json!({"done": "d", "state": "s", "next": "n"})))
            .await
            .unwrap();
        let (is_error, texts) = result_texts(&result);
        assert!(!is_error, "{texts:?}");
        let [summary] = texts.as_slice() else { panic!("{texts:?}") };
        assert!(summary.starts_with("path: stories/story-seven.md\nentry: ### "), "{summary}");
        assert!(summary.ends_with(&format!(" · me/sc-7/x\ncwd: {}\nbranch: me/sc-7/x", repo.display())), "{summary}");
        let body = std::fs::read_to_string(tmp.path().join("notes/stories/story-seven.md")).unwrap();
        assert_eq!(arc_context::latest_sessions(&body, 3).len(), 1);
    }

    #[tokio::test]
    async fn log_session_without_a_story_is_a_tool_error() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let mcp = server_with_story(tmp.path(), outside.path().to_path_buf()).await;
        let result = mcp
            .log_session(log_args(json!({"done": "d", "state": "s", "next": "n"})))
            .await
            .unwrap();
        let (is_error, texts) = result_texts(&result);
        assert!(is_error);
        assert!(texts[0].contains("pass a story id"), "{texts:?}");
    }

    fn cwd_args(cwd: Option<&Path>) -> Parameters<CurrentStoryArgs> {
        Parameters(serde_json::from_value(json!({"cwd": cwd.map(|p| p.display().to_string())})).unwrap())
    }

    #[tokio::test]
    async fn current_story_reports_the_context_or_says_there_is_none() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let mcp = server_with_story(tmp.path(), outside.path().to_path_buf()).await;
        let (is_error, texts) = result_texts(&mcp.current_story(cwd_args(None)).await.unwrap());
        assert!(!is_error);
        assert_eq!(
            texts,
            [format!("no story for this branch\ncwd: {}\nbranch: (not a git repository)", outside.path().display())]
        );

        let Some(repo) = git_repo_on(tmp.path(), "me/sc-7/x") else { return };
        let mcp = ArcMcp::new(config_in(tmp.path()), repo.clone());
        mcp.log_session(log_args(json!({"done": "d", "state": "s", "next": "n"})))
            .await
            .unwrap();
        let (is_error, texts) = result_texts(&mcp.current_story(cwd_args(None)).await.unwrap());
        assert!(!is_error, "{texts:?}");
        let mut cache = mcp.read_cache().await;
        let expected = arc_context::current_context(&repo, &mcp.config, &mut cache).await.unwrap().unwrap();
        assert_eq!(serde_json::from_str::<Value>(&texts[0]).unwrap(), serde_json::to_value(&expected).unwrap());
        assert_eq!(texts[1], format!("cwd: {}\nbranch: me/sc-7/x", repo.display()));
        assert_eq!(expected.note, Some(rel("stories/story-seven.md")));
        assert_eq!(expected.sessions.len(), 1);
    }

    #[tokio::test]
    async fn session_tools_resolve_against_the_cwd_argument() {
        let tmp = tempfile::tempdir().unwrap();
        let Some(worktree) = git_repo_on(tmp.path(), "me/sc-7/x") else { return };
        let server_dir = tmp.path().join("checkout");
        std::fs::create_dir_all(&server_dir).unwrap();
        let mcp = server_with_story(tmp.path(), server_dir.clone()).await;

        let (is_error, texts) = result_texts(&mcp.current_story(cwd_args(Some(&worktree))).await.unwrap());
        assert!(!is_error, "{texts:?}");
        assert_eq!(serde_json::from_str::<Value>(&texts[0]).unwrap()["story"]["id"], 7);

        let cwd = worktree.display().to_string();
        let result = mcp
            .log_session(log_args(json!({"cwd": cwd, "done": "d", "state": "s", "next": "n"})))
            .await
            .unwrap();
        let (is_error, texts) = result_texts(&result);
        assert!(!is_error, "{texts:?}");
        assert!(texts[0].ends_with(&format!("cwd: {cwd}\nbranch: me/sc-7/x")), "{texts:?}");

        let (_, texts) = result_texts(&mcp.current_story(cwd_args(None)).await.unwrap());
        assert!(texts[0].starts_with("no story for this branch"), "{texts:?}");

        for bad in ["relative/dir", "/definitely/not/a/dir"] {
            let (is_error, texts) = result_texts(
                &mcp.current_story(Parameters(serde_json::from_value(json!({"cwd": bad})).unwrap())).await.unwrap(),
            );
            assert!(is_error && texts[0].contains("`cwd`"), "{texts:?}");
            let (is_error, texts) = result_texts(
                &mcp.log_session(log_args(json!({"cwd": bad, "story_id": 7, "done": "d", "state": "s", "next": "n"})))
                    .await
                    .unwrap(),
            );
            assert!(is_error && texts[0].contains("`cwd`"), "{texts:?}");
        }
    }

    fn branch_args(branch: &str) -> Parameters<CurrentStoryArgs> {
        Parameters(serde_json::from_value(json!({"branch": branch})).unwrap())
    }

    #[tokio::test]
    async fn session_tools_resolve_against_the_branch_argument() {
        let tmp = tempfile::tempdir().unwrap();
        let mcp = ArcMcp::remote(server_with_story(tmp.path(), tmp.path().to_path_buf()).await.config);

        let (is_error, texts) = result_texts(&mcp.current_story(branch_args("me/sc-7/x")).await.unwrap());
        assert!(!is_error, "{texts:?}");
        assert_eq!(serde_json::from_str::<Value>(&texts[0]).unwrap()["story"]["id"], 7);
        assert_eq!(texts[1], "branch: me/sc-7/x");

        let result = mcp
            .log_session(log_args(json!({"branch": "me/sc-7/x", "done": "d", "state": "s", "next": "n"})))
            .await
            .unwrap();
        let (is_error, texts) = result_texts(&result);
        assert!(!is_error, "{texts:?}");
        assert!(texts[0].starts_with("path: stories/story-seven.md\nentry: ### "), "{texts:?}");
        assert!(texts[0].ends_with(" · me/sc-7/x\nbranch: me/sc-7/x"), "{texts:?}");

        let (is_error, texts) = result_texts(&mcp.current_story(branch_args("unlinked")).await.unwrap());
        assert!(!is_error);
        assert_eq!(texts, ["no story for this branch\nbranch: unlinked"]);
    }

    #[tokio::test]
    async fn remote_servers_refuse_to_guess_the_checkout() {
        let tmp = tempfile::tempdir().unwrap();
        let mcp = ArcMcp::remote(server_with_story(tmp.path(), tmp.path().to_path_buf()).await.config);
        let (is_error, texts) = result_texts(&mcp.current_story(cwd_args(None)).await.unwrap());
        assert!(is_error && texts[0].contains("pass your git branch name as `branch`"), "{texts:?}");
        let (is_error, texts) =
            result_texts(&mcp.log_session(log_args(json!({"done": "d", "state": "s", "next": "n"}))).await.unwrap());
        assert!(is_error && texts[0].contains("`branch`") && texts[0].contains("`story_id`"), "{texts:?}");
        let instructions = mcp.get_info().instructions.unwrap();
        assert!(instructions.contains("never `cwd`"), "{instructions}");
    }

    #[test]
    fn lookup_refreshes_are_claimed_once_per_interval() {
        let tmp = tempfile::tempdir().unwrap();
        let mcp = ArcMcp::remote(config_in(tmp.path()));
        let start = Instant::now();
        assert!(mcp.claim_lookup_refresh(start));
        assert!(!mcp.clone().claim_lookup_refresh(start + LOOKUP_REFRESH_INTERVAL / 2));
        assert!(mcp.claim_lookup_refresh(start + LOOKUP_REFRESH_INTERVAL));
        assert!(!mcp.claim_lookup_refresh(start + LOOKUP_REFRESH_INTERVAL));
    }

    #[tokio::test]
    async fn remote_servers_reject_cwd() {
        let tmp = tempfile::tempdir().unwrap();
        let mcp = ArcMcp::remote(server_with_story(tmp.path(), tmp.path().to_path_buf()).await.config);
        let cwd = tmp.path().display().to_string();
        let (is_error, texts) = result_texts(&mcp.current_story(cwd_args(Some(tmp.path()))).await.unwrap());
        assert!(is_error, "{texts:?}");
        assert_eq!(texts, ["this server can't see your filesystem; pass `branch`"]);
        let (is_error, texts) = result_texts(
            &mcp.log_session(log_args(json!({"cwd": cwd, "story_id": 7, "done": "d", "state": "s", "next": "n"})))
                .await
                .unwrap(),
        );
        assert!(is_error && texts[0].contains("can't see your filesystem"), "{texts:?}");
        assert!(Locality::Remote.checkout(Some(&cwd), None).is_err());
    }

    #[tokio::test]
    async fn remote_log_session_takes_a_bare_story_id() {
        let tmp = tempfile::tempdir().unwrap();
        let mcp = ArcMcp::remote(server_with_story(tmp.path(), tmp.path().to_path_buf()).await.config);
        let (is_error, texts) = result_texts(
            &mcp.log_session(log_args(json!({"story_id": "sc-7", "done": "d", "state": "s", "next": "n"})))
                .await
                .unwrap(),
        );
        assert!(!is_error, "{texts:?}");
        let [path, entry] = texts[0].lines().collect::<Vec<_>>()[..] else { panic!("{texts:?}") };
        assert_eq!(path, "path: stories/story-seven.md");
        assert!(entry.starts_with("entry: ### ") && !entry.contains(" · "), "{entry}");
    }

    #[tokio::test]
    async fn checkout_arguments_are_exclusive_and_validated() {
        let tmp = tempfile::tempdir().unwrap();
        let mcp = server_with_story(tmp.path(), tmp.path().to_path_buf()).await;
        let both = json!({"cwd": tmp.path().display().to_string(), "branch": "me/sc-7/x"});
        let (is_error, texts) =
            result_texts(&mcp.current_story(Parameters(serde_json::from_value(both).unwrap())).await.unwrap());
        assert!(is_error && texts[0].contains("not both"), "{texts:?}");
        let (is_error, texts) = result_texts(&mcp.current_story(branch_args("")).await.unwrap());
        assert!(is_error && texts[0].contains("`branch`"), "{texts:?}");
        assert!(mcp.get_info().instructions.unwrap().contains("as `cwd`"));
    }

    #[tokio::test]
    async fn append_note_tool_appends_under_a_section() {
        let tmp = tempfile::tempdir().unwrap();
        let mcp = ArcMcp::new(config_in(tmp.path()), tmp.path().to_path_buf());
        let conn = arc_index::open(&tmp.path().join("cache")).unwrap();
        let notes = tmp.path().join("notes");
        create_note(&Vault::new(&conn, &notes, &tmp.path().join("cache")), "N", "body", vec![]).unwrap();
        let append = |content: &str, section: Option<&str>| {
            mcp.append_note(Parameters(AppendNoteArgs {
                id: None,
                path: Some("n.md".into()),
                content: content.into(),
                section: section.map(Into::into),
            }))
        };
        let (_, texts) = result_texts(&append("- one", Some("Log")).await.unwrap());
        assert_eq!(texts, vec!["appended to n.md under ## Log"]);
        let (_, texts) = result_texts(&append("tail", None).await.unwrap());
        assert_eq!(texts, vec!["appended to n.md"]);
        append("- two", Some("## Log")).await.unwrap();
        let (is_error, texts) = result_texts(&append("x", Some(" ")).await.unwrap());
        assert!(is_error, "{texts:?}");
        let body = std::fs::read_to_string(tmp.path().join("notes/n.md")).unwrap();
        assert!(body.ends_with("# N\n\nbody\n\n## Log\n\n- one\n\ntail\n\n- two\n"), "{body}");
    }

}
