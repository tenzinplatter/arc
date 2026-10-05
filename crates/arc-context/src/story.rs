use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, bail};
use arc_core::StoryId;
use arc_shortcut::api::story::Story;
use arc_shortcut::api::user::CurrentUser;
use arc_shortcut::{ApiClient, Cache, fetch_story};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const FETCH_TIMEOUT: Duration = Duration::from_secs(3);
pub const FETCH_RETRY_AFTER: chrono::TimeDelta = chrono::TimeDelta::minutes(10);
const FETCH_FAILURES_FILE: &str = "story-fetch-failures.json";

async fn timed<T>(fut: impl Future<Output = anyhow::Result<T>>) -> anyhow::Result<T> {
    tokio::time::timeout(FETCH_TIMEOUT, fut)
        .await
        .map_err(|_| anyhow!("Shortcut didn't answer within {}s", FETCH_TIMEOUT.as_secs()))?
}

pub async fn lookup_story(
    cache: &mut Cache,
    api_token: &str,
    id: StoryId,
) -> anyhow::Result<Story> {
    if let Some(story) = cache.find_story(id) {
        return Ok(story.clone());
    }
    timed(async {
        let client = ApiClient::from_token_and_cache(api_token, cache).await?;
        fetch_story(cache, &client, id).await
    })
    .await
}

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
struct FetchFailures(HashMap<StoryId, DateTime<Utc>>);

impl FetchFailures {
    fn path(cache_dir: &Path) -> PathBuf {
        cache_dir.join(FETCH_FAILURES_FILE)
    }

    fn read(cache_dir: &Path) -> Self {
        std::fs::read_to_string(Self::path(cache_dir))
            .ok()
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default()
    }

    fn write(&self, cache_dir: &Path) -> anyhow::Result<()> {
        std::fs::create_dir_all(cache_dir)?;
        let mut tmp = tempfile::NamedTempFile::new_in(cache_dir)?;
        tmp.write_all(&serde_json::to_vec(self)?)?;
        tmp.persist(Self::path(cache_dir))?;
        Ok(())
    }

    fn recent(&self, id: StoryId, now: DateTime<Utc>) -> bool {
        self.0
            .get(&id)
            .is_some_and(|&failed| now - failed < FETCH_RETRY_AFTER)
    }

    fn after(&self, id: StoryId, failed: bool, now: DateTime<Utc>) -> Self {
        Self(
            self.0
                .iter()
                .map(|(&other, &at)| (other, at))
                .filter(|&(other, at)| other != id && now - at < FETCH_RETRY_AFTER)
                .chain(failed.then_some((id, now)))
                .collect(),
        )
    }
}

pub async fn lookup_story_backing_off(
    cache: &mut Cache,
    api_token: &str,
    cache_dir: &Path,
    id: StoryId,
) -> anyhow::Result<Story> {
    if let Some(story) = cache.find_story(id) {
        return Ok(story.clone());
    }
    let now = Utc::now();
    let failures = FetchFailures::read(cache_dir);
    if failures.recent(id, now) {
        bail!("fetching story {id} failed recently; not retrying yet");
    }
    let looked_up = lookup_story(cache, api_token, id).await;
    let updated = failures.after(id, looked_up.is_err(), now);
    if updated != failures {
        updated.write(cache_dir)?;
    }
    looked_up
}

pub async fn lookup_user(cache: &mut Cache, api_token: &str) -> anyhow::Result<CurrentUser> {
    if let Some(user) = cache.user() {
        return Ok(user);
    }
    timed(ApiClient::from_token_and_cache(api_token, cache))
        .await
        .map(|client| client.user().clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(minutes: i64) -> DateTime<Utc> {
        DateTime::UNIX_EPOCH + chrono::TimeDelta::minutes(minutes)
    }

    #[test]
    fn failures_expire_and_clear_on_success() {
        let failures = FetchFailures::default().after(7.into(), true, at(0));
        assert!(failures.recent(7.into(), at(9)));
        assert!(!failures.recent(7.into(), at(10)));
        assert!(!failures.recent(8.into(), at(1)));

        let failures = failures.after(8.into(), true, at(5));
        assert_eq!(failures.0.len(), 2);
        let failures = failures.after(8.into(), false, at(11));
        assert_eq!(failures, FetchFailures::default());
    }

    #[test]
    fn failures_round_trip_through_the_cache_dir() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(FetchFailures::read(tmp.path()), FetchFailures::default());
        let failures = FetchFailures::default().after(7.into(), true, at(0));
        failures.write(tmp.path()).unwrap();
        assert_eq!(FetchFailures::read(tmp.path()), failures);
        std::fs::write(FetchFailures::path(tmp.path()), "not json").unwrap();
        assert_eq!(FetchFailures::read(tmp.path()), FetchFailures::default());
    }

    #[tokio::test]
    async fn recent_failure_skips_the_fetch() {
        let tmp = tempfile::tempdir().unwrap();
        FetchFailures::default()
            .after(7.into(), true, Utc::now())
            .write(tmp.path())
            .unwrap();
        let mut cache = Cache::default().with_cache_dir(tmp.path().to_path_buf());
        let err = lookup_story_backing_off(&mut cache, "", tmp.path(), 7.into())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("failed recently"), "{err}");
    }
}
