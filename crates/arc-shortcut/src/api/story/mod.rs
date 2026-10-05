use crate::api::{
    ApiClient, branch::Branch, iteration::Iteration, story::comment::StoryComment, user::MemberId,
    workflow::WorkflowStateId,
};
use anyhow::Context;
use arc_core::{EpicId, IterationId, StoryId};
use serde::{Deserialize, Serialize};
use slugify::slugify;

pub mod comment;

#[derive(Deserialize, Serialize, Clone, PartialEq, Eq, Debug, Default)]
pub struct Story {
    pub branches: Vec<Branch>,
    pub completed: bool,
    pub comments: Vec<StoryComment>,
    pub description: String,
    pub epic_id: Option<EpicId>,
    pub id: StoryId,
    pub iteration_id: Option<IterationId>,
    pub name: String,
    pub app_url: String,
    #[serde(default)]
    pub workflow_state_id: Option<WorkflowStateId>,
    #[serde(default)]
    pub story_type: String,
    #[serde(default)]
    pub owner_ids: Vec<MemberId>,
    #[serde(default)]
    pub estimate: Option<i32>,
}

#[derive(Deserialize)]
struct StorySearchResults {
    data: Vec<Story>,
    next: Option<String>,
}

impl ApiClient {
    pub(crate) async fn search_stories_all_pages(&self, query: &str) -> anyhow::Result<Vec<Story>> {
        let mut all = Vec::new();
        let mut next_path: Option<String> = None;

        loop {
            let response = match &next_path {
                None => {
                    let params = [("query", query), ("page_size", "250"), ("detail", "full")];
                    self.get_with_query("search/stories", &params).await?
                }
                Some(path) => self.get_next_page(path).await?,
            };

            let page: StorySearchResults = response
                .json()
                .await
                .context("Failed to parse search/stories response")?;

            all.extend(page.data);
            match page.next {
                Some(n) => next_path = Some(n),
                None => break,
            }
        }

        Ok(all)
    }

    pub async fn get_story(&self, id: StoryId) -> anyhow::Result<Story> {
        let response = self
            .get(&format!("stories/{id}"))
            .await?
            .error_for_status()?;
        response
            .json()
            .await
            .with_context(|| format!("Failed to parse story {id}"))
    }

    pub async fn update_story_description(
        &self,
        story_id: StoryId,
        new_description: String,
    ) -> anyhow::Result<()> {
        let body = serde_json::json!({
            "description": new_description,
        });

        self.put_with_body(&format!("stories/{}", story_id), &body)
            .await?
            .error_for_status()?;
        Ok(())
    }
}

impl Story {
    pub fn tmux_session_name(name: &str) -> String {
        let story_slug = slugify!(name);
        format!("scn--{}", story_slug)
    }
}

pub fn iteration_of<'a>(story: &Story, iterations: &'a [Iteration]) -> Option<&'a Iteration> {
    get_story_associated_iteration(story.iteration_id, iterations)
}

pub fn get_story_associated_iteration<'a>(
    iteration_id: Option<IterationId>,
    iterations: impl IntoIterator<Item = &'a Iteration>,
) -> Option<&'a Iteration> {
    let iteration_id = iteration_id?;
    iterations.into_iter().find(|it| it.id == iteration_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserializes_full_story_payload() {
        let json = r#"{
            "app_url": "https://app.shortcut.com/org/story/16727",
            "archived": false,
            "branches": [{"id": 1, "name": "tenzin/sc-16727/optical-flow", "url": "https://github.com/x"}],
            "comments": [{
                "id": 99,
                "author_id": null,
                "created_at": "2026-09-30T12:00:00Z",
                "deleted": false,
                "position": 1,
                "text": "looks good"
            }],
            "completed": false,
            "description": "desc",
            "entity_type": "story",
            "epic_id": 12,
            "estimate": 3,
            "id": 16727,
            "iteration_id": null,
            "labels": [],
            "name": "Optical flow",
            "owner_ids": ["12345678-1234-1234-1234-123456789abc"],
            "story_type": "feature",
            "workflow_id": 500000001,
            "workflow_state_id": 500000007
        }"#;
        let story: Story = serde_json::from_str(json).unwrap();
        assert_eq!(story.id, 16727.into());
        assert_eq!(story.workflow_state_id, Some(500000007.into()));
        assert_eq!(story.story_type, "feature");
        assert_eq!(story.owner_ids.len(), 1);
        assert_eq!(story.estimate, Some(3));
        assert_eq!(story.comments[0].id, 99);
        assert_eq!(story.comments[0].author_id, None);
        assert!(story.comments[0].created_at.is_some());
    }

    #[test]
    fn deserializes_story_without_new_fields() {
        let json = r#"{
            "app_url": "https://app.shortcut.com/org/story/1",
            "branches": [],
            "comments": [],
            "completed": true,
            "description": "",
            "epic_id": null,
            "id": 1,
            "iteration_id": 2,
            "name": "Old",
            "estimate": null
        }"#;
        let story: Story = serde_json::from_str(json).unwrap();
        assert_eq!(story.workflow_state_id, None);
        assert_eq!(story.story_type, "");
        assert!(story.owner_ids.is_empty());
        assert_eq!(story.estimate, None);
    }
}
