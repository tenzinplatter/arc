use chrono::NaiveDate;
use futures::future::join_all;
use serde::{Deserialize, Serialize};

use crate::api::{ApiClient, story::Story};
use arc_core::{IterationId, dbg_file};

#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum IterationStatus {
    Unstarted,
    Started,
    Done,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct Iteration {
    pub id: IterationId,
    pub name: String,
    pub start_date: NaiveDate,
    pub end_date: NaiveDate,
    pub app_url: String,
    pub status: IterationStatus,
}

impl ApiClient {
    pub async fn get_current_iterations(&self) -> anyhow::Result<Vec<Iteration>> {
        let all = self.get_all_iterations().await?;
        Ok(all
            .into_iter()
            .filter(|it| it.status == IterationStatus::Started)
            .collect())
    }

    pub async fn get_all_iterations(&self) -> anyhow::Result<Vec<Iteration>> {
        let response = self.get("iterations").await?;
        Ok(response.json().await?)
    }

    pub async fn get_owned_iteration_stories(
        &self,
        iteration_ids: Vec<IterationId>,
    ) -> anyhow::Result<Vec<Story>> {
        let iteration_stories = join_all(iteration_ids.iter().map(|id| async move {
            let query = format!("iteration:{} owner:{}", id, self.user.mention_name);
            self.search_stories_all_pages(&query).await
        }))
        .await
        .into_iter()
        .filter_map(|res| match res {
            Ok(stories) => Some(stories),
            Err(e) => {
                dbg_file!("Failed to fetch iteration stories with error: {}", e);
                None
            }
        })
        .flatten()
        .collect();

        Ok(iteration_stories)
    }
}
