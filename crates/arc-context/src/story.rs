use std::time::Duration;

use anyhow::anyhow;
use arc_core::StoryId;
use arc_shortcut::api::story::Story;
use arc_shortcut::api::user::CurrentUser;
use arc_shortcut::{ApiClient, Cache, fetch_story};

pub const FETCH_TIMEOUT: Duration = Duration::from_secs(3);

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

pub async fn lookup_user(cache: &mut Cache, api_token: &str) -> anyhow::Result<CurrentUser> {
    if let Some(user) = cache.user() {
        return Ok(user);
    }
    timed(ApiClient::from_token_and_cache(api_token, cache))
        .await
        .map(|client| client.user().clone())
}
