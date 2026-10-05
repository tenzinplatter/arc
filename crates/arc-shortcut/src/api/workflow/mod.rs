use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::api::ApiClient;

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct WorkflowState {
    pub id: i64,
    pub name: String,
    /// "unstarted" | "started" | "done"
    #[serde(rename = "type")]
    pub state_type: String,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct Workflow {
    pub id: i64,
    pub name: String,
    pub states: Vec<WorkflowState>,
}

impl ApiClient {
    pub async fn get_workflows(&self) -> anyhow::Result<Vec<Workflow>> {
        let response = self.get("workflows").await?.error_for_status()?;
        response
            .json()
            .await
            .context("Failed to parse workflows response")
    }
}
