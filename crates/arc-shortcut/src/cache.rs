use std::{
    collections::HashMap,
    env,
    io::Write,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use tokio::{fs::{self, File}, io::AsyncReadExt};
use crate::api::{
    ApiClient, epic::EpicSlim, iteration::Iteration, story::Story, user::MemberId,
    workflow::WorkflowStateId,
};
use arc_core::{StoryId, dbg_file};

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct Cache {
    pub current_iterations: Option<Vec<Iteration>>,
    pub iterations: Vec<Iteration>,
    pub iteration_stories: Option<Vec<Story>>,
    pub epics: Vec<EpicSlim>,
    pub active_story: Option<Story>,
    pub user_id: Option<MemberId>,
    #[serde(default)]
    pub user_mention_name: Option<String>,
    pub cache_dir: PathBuf,
    /// Stories fetched individually, e.g. outside the current iteration.
    #[serde(default)]
    pub stories_by_id: HashMap<StoryId, Story>,
    /// Workflow state id -> name.
    #[serde(default)]
    pub workflow_states: HashMap<WorkflowStateId, String>,
    /// Member id -> mention name.
    #[serde(default)]
    pub members: HashMap<MemberId, String>,
}

impl Default for Cache {
    fn default() -> Self {
        Self {
            current_iterations: None,
            iteration_stories: None,
            iterations: Vec::new(),
            epics: Vec::new(),
            user_id: None,
            user_mention_name: None,
            active_story: None,
            cache_dir: Self::default_cache_dir(),
            stories_by_id: HashMap::new(),
            workflow_states: HashMap::new(),
            members: HashMap::new(),
        }
    }
}

impl Cache {
    pub fn current_iterations_ref(&self) -> Option<Vec<&Iteration>> {
        self.current_iterations.as_ref().map(|v| v.iter().collect())
    }

    /// Looks in iteration_stories, then stories_by_id, then active_story (a
    /// frozen copy, so least likely to be fresh).
    pub fn find_story(&self, id: StoryId) -> Option<&Story> {
        self.iteration_stories
            .as_ref()
            .and_then(|stories| stories.iter().find(|s| s.id == id))
            .or_else(|| self.stories_by_id.get(&id))
            .or_else(|| self.active_story.as_ref().filter(|s| s.id == id))
    }

    pub fn state_name(&self, id: Option<WorkflowStateId>) -> Option<&str> {
        self.workflow_states.get(&id?).map(String::as_str)
    }

    pub fn member_name(&self, id: &MemberId) -> Option<&str> {
        self.members.get(id).map(String::as_str)
    }

    fn default_cache_dir() -> PathBuf {
        let mut base = env::home_dir().expect("Couldn't find home dir");
        base.push(".cache");
        base.push("arc");
        base
    }

    pub fn get_cache_file(mut cache_dir: PathBuf) -> PathBuf {
        cache_dir.push("cache.json");
        cache_dir
    }

    pub async fn read(cache_dir: PathBuf) -> Self {
        dbg_file!("Using {} as cache_dir", cache_dir.display());

        let cache_file = Self::get_cache_file(cache_dir);

        if let Some(parent) = cache_file.parent()
            && !parent.exists()
            && let Err(e) = fs::create_dir_all(parent).await
        {
            dbg_file!(
                "Failed to create cache dir parent at: {} with err: {}",
                parent.display(),
                e
            );

            return Self::default();
        }

        let contents = match read_file(&cache_file).await {
            Ok(contents) => contents,
            Err(_) => {
                return Self::default();
            }
        };

        match serde_json::from_str::<Cache>(&contents) {
            Ok(cache) => cache,
            Err(e) => {
                dbg_file!("Failed to parse cache at {}: {}", cache_file.display(), e);
                if cache_file.is_file() {
                    let _ = fs::rename(&cache_file, Self::bad_cache_file(&cache_file)).await;
                }
                Self::default()
            }
        }
    }

    fn bad_cache_file(cache_file: &Path) -> PathBuf {
        cache_file.with_extension("json.bad")
    }

    /// Merges the on-disk cache into this one, then atomically replaces the file.
    /// `stories_by_id`, `workflow_states` and `members` are unioned with this
    /// cache's entries winning on conflict; every other field is taken from
    /// `self` (except user id/name, filled from disk when missing).
    pub async fn write(&self) -> anyhow::Result<()> {
        let cache_file = Self::get_cache_file(self.cache_dir.clone());
        let mut merged = self.clone();
        if let Ok(contents) = read_file(&cache_file).await
            && let Ok(disk) = serde_json::from_str::<Cache>(&contents)
        {
            merged.absorb(disk);
        }

        let bytes = serde_json::to_vec(&merged)?;
        let dir = self.cache_dir.clone();
        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            std::fs::create_dir_all(&dir)?;
            let mut tmp = tempfile::NamedTempFile::new_in(&dir)?;
            tmp.write_all(&bytes)?;
            tmp.persist(&cache_file)?;
            Ok(())
        })
        .await??;

        Ok(())
    }

    fn absorb(&mut self, disk: Cache) {
        for (k, v) in disk.stories_by_id {
            self.stories_by_id.entry(k).or_insert(v);
        }
        for (k, v) in disk.workflow_states {
            self.workflow_states.entry(k).or_insert(v);
        }
        for (k, v) in disk.members {
            self.members.entry(k).or_insert(v);
        }
        if self.user_id.is_none() {
            self.user_id = disk.user_id;
            self.user_mention_name = self.user_mention_name.take().or(disk.user_mention_name);
        }
    }
}

/// Returns the cached story, or fetches it and stores it in `stories_by_id`.
pub async fn fetch_story(cache: &mut Cache, client: &ApiClient, id: StoryId) -> anyhow::Result<Story> {
    if let Some(story) = cache.find_story(id) {
        return Ok(story.clone());
    }

    let story = client.get_story(id).await?;
    cache.stories_by_id.insert(id, story.clone());
    cache.write().await?;
    Ok(story)
}

/// Fills `workflow_states` and `members` if they're empty. Each is persisted as
/// soon as it's fetched, so one failing doesn't lose the other.
pub async fn ensure_lookups(cache: &mut Cache, client: &ApiClient) -> anyhow::Result<()> {
    if cache.workflow_states.is_empty() {
        refresh_workflow_states(cache, client).await?;
    }
    if cache.members.is_empty() {
        refresh_members(cache, client).await?;
    }
    Ok(())
}

/// Refetches `workflow_states` and `members` unconditionally, e.g. after a lookup miss.
pub async fn refresh_lookups(cache: &mut Cache, client: &ApiClient) -> anyhow::Result<()> {
    let states = refresh_workflow_states(cache, client).await;
    let members = refresh_members(cache, client).await;
    states.and(members)
}

async fn refresh_workflow_states(cache: &mut Cache, client: &ApiClient) -> anyhow::Result<()> {
    cache.workflow_states = client
        .get_workflows()
        .await?
        .into_iter()
        .flat_map(|w| w.states)
        .map(|s| (s.id, s.name))
        .collect();
    cache.write().await
}

async fn refresh_members(cache: &mut Cache, client: &ApiClient) -> anyhow::Result<()> {
    cache.members = client
        .get_members()
        .await?
        .into_iter()
        .map(|m| (m.id, m.mention_name))
        .collect();
    cache.write().await
}

async fn read_file(file: &PathBuf) -> anyhow::Result<String> {
    let mut f = File::open(file).await?;
    let mut buf = String::new();
    f.read_to_string(&mut buf).await?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn story(id: i32, name: &str) -> Story {
        Story {
            id: id.into(),
            name: name.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn old_cache_shape_deserializes() {
        let json = r#"{
            "current_iterations": null,
            "iterations": [],
            "iteration_stories": [{
                "branches": [],
                "completed": false,
                "comments": [{"author_id": "12345678-1234-1234-1234-123456789abc", "deleted": false, "position": 1, "text": "hi"}],
                "description": "d",
                "epic_id": null,
                "id": 7,
                "iteration_id": 3,
                "name": "old story",
                "app_url": "https://app.shortcut.com/x/story/7"
            }],
            "epics": [],
            "active_story": null,
            "user_id": null,
            "cache_dir": "/tmp/arc"
        }"#;
        let cache: Cache = serde_json::from_str(json).unwrap();
        assert!(cache.stories_by_id.is_empty());
        assert!(cache.workflow_states.is_empty());
        assert!(cache.members.is_empty());
        let s = cache.find_story(7.into()).unwrap();
        assert_eq!(s.story_type, "");
        assert_eq!(s.workflow_state_id, None);
        assert!(s.owner_ids.is_empty());
    }

    #[test]
    fn cache_round_trips_new_fields() {
        let mut cache = Cache::default();
        cache.stories_by_id.insert(5.into(), story(5, "five"));
        cache.workflow_states.insert(500.into(), "In Progress".into());
        let id = MemberId::from(Uuid::nil());
        cache.members.insert(id, "tenzin".into());

        let back: Cache = serde_json::from_str(&serde_json::to_string(&cache).unwrap()).unwrap();
        assert_eq!(back.find_story(5.into()).unwrap().name, "five");
        assert_eq!(back.state_name(Some(500.into())), Some("In Progress"));
        assert_eq!(back.state_name(None), None);
        assert_eq!(back.member_name(&id), Some("tenzin"));
    }

    #[test]
    fn find_story_precedence() {
        let mut cache = Cache {
            active_story: Some(story(1, "active")),
            iteration_stories: Some(vec![story(1, "iteration"), story(2, "iteration")]),
            ..Default::default()
        };
        cache.stories_by_id.insert(1.into(), story(1, "by_id"));
        cache.stories_by_id.insert(2.into(), story(2, "by_id"));
        cache.stories_by_id.insert(3.into(), story(3, "by_id"));

        cache.active_story = Some(story(9, "active"));
        cache.stories_by_id.insert(9.into(), story(9, "by_id"));

        assert_eq!(cache.find_story(1.into()).unwrap().name, "iteration");
        assert_eq!(cache.find_story(2.into()).unwrap().name, "iteration");
        assert_eq!(cache.find_story(3.into()).unwrap().name, "by_id");
        assert_eq!(cache.find_story(9.into()).unwrap().name, "by_id");
        assert!(cache.find_story(4.into()).is_none());

        cache.stories_by_id.remove(&9.into());
        assert_eq!(cache.find_story(9.into()).unwrap().name, "active");
    }

    fn cache_in(dir: &Path) -> Cache {
        Cache {
            cache_dir: dir.to_path_buf(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn write_then_read_round_trips_without_leftover_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let mut cache = cache_in(dir.path());
        cache.workflow_states.insert(1.into(), "Done".into());
        cache.write().await.unwrap();

        let back = Cache::read(dir.path().to_path_buf()).await;
        assert_eq!(back.state_name(Some(1.into())), Some("Done"));
        let entries: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(entries.len(), 1);
    }

    #[tokio::test]
    async fn corrupt_cache_is_moved_aside() {
        let dir = tempfile::tempdir().unwrap();
        let file = Cache::get_cache_file(dir.path().to_path_buf());
        std::fs::write(&file, "{ not json").unwrap();

        let cache = Cache::read(dir.path().to_path_buf()).await;
        assert!(cache.stories_by_id.is_empty());
        assert!(!file.exists());
        let bad = dir.path().join("cache.json.bad");
        assert_eq!(std::fs::read_to_string(bad).unwrap(), "{ not json");
    }

    #[tokio::test]
    async fn write_merges_disk_lookups_and_keeps_writer_fields() {
        let dir = tempfile::tempdir().unwrap();
        let other_id = MemberId::from(Uuid::from_u128(1));
        let mut other = cache_in(dir.path());
        other.active_story = Some(story(1, "other active"));
        other.user_id = Some(other_id);
        other.user_mention_name = Some("other".into());
        other.stories_by_id.insert(1.into(), story(1, "disk"));
        other.stories_by_id.insert(2.into(), story(2, "disk"));
        other.workflow_states.insert(10.into(), "disk state".into());
        other.workflow_states.insert(11.into(), "only disk".into());
        other.members.insert(other_id, "disk member".into());
        other.write().await.unwrap();

        let mut stale = cache_in(dir.path());
        stale.stories_by_id.insert(2.into(), story(2, "mine"));
        stale.workflow_states.insert(10.into(), "my state".into());
        stale.write().await.unwrap();

        let back = Cache::read(dir.path().to_path_buf()).await;
        assert!(back.active_story.is_none());
        assert_eq!(back.user_id, Some(other_id));
        assert_eq!(back.user_mention_name.as_deref(), Some("other"));
        assert_eq!(back.stories_by_id[&1.into()].name, "disk");
        assert_eq!(back.stories_by_id[&2.into()].name, "mine");
        assert_eq!(back.state_name(Some(10.into())), Some("my state"));
        assert_eq!(back.state_name(Some(11.into())), Some("only disk"));
        assert_eq!(back.member_name(&other_id), Some("disk member"));
    }

    fn golden_full_cache_json() -> serde_json::Value {
        serde_json::json!({
            "current_iterations": [{
                "id": 3,
                "name": "Sprint 3",
                "start_date": "2026-01-05",
                "end_date": "2026-01-18",
                "app_url": "https://app.shortcut.com/x/iteration/3",
                "status": "started"
            }],
            "iterations": [{
                "id": 2,
                "name": "Sprint 2",
                "start_date": "2025-12-22",
                "end_date": "2026-01-04",
                "app_url": "https://app.shortcut.com/x/iteration/2",
                "status": "done"
            }],
            "iteration_stories": [{
                "branches": [{"id": 11, "name": "sc-7-feature"}],
                "completed": false,
                "comments": [{
                    "id": 99,
                    "author_id": "12345678-1234-1234-1234-123456789abc",
                    "created_at": "2026-01-06T10:00:00Z",
                    "deleted": false,
                    "position": 1,
                    "text": "hello"
                }],
                "description": "desc",
                "epic_id": 40,
                "id": 7,
                "iteration_id": 3,
                "name": "story seven",
                "app_url": "https://app.shortcut.com/x/story/7",
                "workflow_state_id": 500,
                "story_type": "feature",
                "owner_ids": ["12345678-1234-1234-1234-123456789abc"],
                "estimate": 3
            }],
            "epics": [{
                "id": 40,
                "name": "Epic forty",
                "app_url": "https://app.shortcut.com/x/epic/40",
                "owner_ids": ["12345678-1234-1234-1234-123456789abc"],
                "created_at": "2025-11-01T00:00:00Z"
            }],
            "active_story": null,
            "user_id": "12345678-1234-1234-1234-123456789abc",
            "user_mention_name": "tenzin",
            "cache_dir": "/tmp/arc-golden",
            "stories_by_id": {
                "5": {
                    "branches": [],
                    "completed": true,
                    "comments": [{
                        "id": 0,
                        "author_id": null,
                        "created_at": null,
                        "deleted": true,
                        "position": 2,
                        "text": null
                    }],
                    "description": "",
                    "epic_id": null,
                    "id": 5,
                    "iteration_id": null,
                    "name": "five",
                    "app_url": "https://app.shortcut.com/x/story/5",
                    "workflow_state_id": null,
                    "story_type": "bug",
                    "owner_ids": [],
                    "estimate": null
                }
            },
            "workflow_states": {"500": "In Progress"},
            "members": {"12345678-1234-1234-1234-123456789abc": "tenzin"}
        })
    }

    fn golden_full_cache() -> Cache {
        let member = MemberId::from(Uuid::parse_str("12345678-1234-1234-1234-123456789abc").unwrap());
        let iteration = |id: i32, name: &str, start: &str, end: &str, status| Iteration {
            id: id.into(),
            name: name.into(),
            start_date: start.parse().unwrap(),
            end_date: end.parse().unwrap(),
            app_url: format!("https://app.shortcut.com/x/iteration/{id}"),
            status,
        };
        let seven = Story {
            branches: vec![serde_json::from_value(serde_json::json!({"id": 11, "name": "sc-7-feature"})).unwrap()],
            completed: false,
            comments: vec![crate::api::story::comment::StoryComment {
                id: 99,
                author_id: Some(member),
                created_at: Some("2026-01-06T10:00:00Z".parse().unwrap()),
                deleted: false,
                position: 1,
                text: Some("hello".into()),
            }],
            description: "desc".into(),
            epic_id: Some(40.into()),
            id: 7.into(),
            iteration_id: Some(3.into()),
            name: "story seven".into(),
            app_url: "https://app.shortcut.com/x/story/7".into(),
            workflow_state_id: Some(500.into()),
            story_type: "feature".into(),
            owner_ids: vec![member],
            estimate: Some(3),
        };
        let five = Story {
            completed: true,
            comments: vec![crate::api::story::comment::StoryComment {
                id: 0,
                author_id: None,
                created_at: None,
                deleted: true,
                position: 2,
                text: None,
            }],
            id: 5.into(),
            name: "five".into(),
            app_url: "https://app.shortcut.com/x/story/5".into(),
            story_type: "bug".into(),
            ..Default::default()
        };
        let mut cache = Cache {
            current_iterations: Some(vec![iteration(
                3,
                "Sprint 3",
                "2026-01-05",
                "2026-01-18",
                crate::api::iteration::IterationStatus::Started,
            )]),
            iterations: vec![iteration(
                2,
                "Sprint 2",
                "2025-12-22",
                "2026-01-04",
                crate::api::iteration::IterationStatus::Done,
            )],
            iteration_stories: Some(vec![seven]),
            epics: vec![EpicSlim {
                id: 40.into(),
                name: "Epic forty".into(),
                app_url: "https://app.shortcut.com/x/epic/40".into(),
                owner_ids: vec![member],
                created_at: "2025-11-01T00:00:00Z".parse().unwrap(),
            }],
            active_story: None,
            user_id: Some(member),
            user_mention_name: Some("tenzin".into()),
            cache_dir: PathBuf::from("/tmp/arc-golden"),
            ..Default::default()
        };
        cache.stories_by_id.insert(5.into(), five);
        cache.workflow_states.insert(500.into(), "In Progress".into());
        cache.members.insert(member, "tenzin".into());
        cache
    }

    #[test]
    fn golden_cache_json_format_is_stable() {
        assert_eq!(serde_json::to_value(golden_full_cache()).unwrap(), golden_full_cache_json());
    }

    #[test]
    fn golden_cache_json_literal_deserializes() {
        let member = MemberId::from(Uuid::parse_str("12345678-1234-1234-1234-123456789abc").unwrap());
        let cache: Cache = serde_json::from_value(golden_full_cache_json()).unwrap();
        assert_eq!(cache.user_id, Some(member));
        assert_eq!(cache.user_mention_name.as_deref(), Some("tenzin"));
        assert_eq!(cache.cache_dir, PathBuf::from("/tmp/arc-golden"));
        assert_eq!(cache.current_iterations.as_ref().unwrap()[0].id, 3.into());
        assert_eq!(cache.iterations[0].id, 2.into());
        assert_eq!(cache.epics[0].id, 40.into());
        assert_eq!(cache.epics[0].owner_ids, vec![member]);
        let seven = cache.find_story(7.into()).unwrap();
        assert_eq!(seven.epic_id, Some(40.into()));
        assert_eq!(seven.iteration_id, Some(3.into()));
        assert_eq!(seven.workflow_state_id, Some(500.into()));
        assert_eq!(seven.owner_ids, vec![member]);
        assert_eq!(seven.comments[0].author_id, Some(member));
        assert_eq!(seven.branches.len(), 1);
        assert_eq!(cache.find_story(5.into()).unwrap().name, "five");
        assert_eq!(cache.state_name(Some(500.into())), Some("In Progress"));
        assert_eq!(cache.member_name(&member), Some("tenzin"));
        assert_eq!(
            serde_json::to_value(&cache).unwrap(),
            golden_full_cache_json()
        );
    }

    #[test]
    fn golden_minimal_cache_json_format_is_stable() {
        let cache = Cache {
            cache_dir: PathBuf::from("/tmp/arc-golden"),
            ..Default::default()
        };
        let expected = serde_json::json!({
            "current_iterations": null,
            "iterations": [],
            "iteration_stories": null,
            "epics": [],
            "active_story": null,
            "user_id": null,
            "user_mention_name": null,
            "cache_dir": "/tmp/arc-golden",
            "stories_by_id": {},
            "workflow_states": {},
            "members": {}
        });
        assert_eq!(serde_json::to_value(&cache).unwrap(), expected);
        let back: Cache = serde_json::from_value(expected).unwrap();
        assert!(back.user_id.is_none());
        assert!(back.stories_by_id.is_empty());
    }
}
