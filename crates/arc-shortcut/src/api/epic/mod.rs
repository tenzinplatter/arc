use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::{ApiClient, user::MemberId};
use arc_core::EpicId;

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct EpicSlim {
    pub id: EpicId,
    pub name: String,
    pub app_url: String,
    pub owner_ids: Vec<MemberId>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Epic {
    pub id: i32,
    pub app_url: String,
    pub completed: bool,
    pub description: String,
    pub name: String,
    pub owner_ids: Vec<Uuid>,
    pub started: bool,
}

impl ApiClient {
    pub async fn get_all_epics_slim(&self, include_description: bool) -> anyhow::Result<Vec<EpicSlim>> {
        let body = serde_json::json!({
            "includes_description": include_description
        });

        let response = self.get_with_body("epics", &body).await?;
        let epics_slim = response.json::<Vec<EpicSlim>>().await?;

        Ok(epics_slim)
    }

}
