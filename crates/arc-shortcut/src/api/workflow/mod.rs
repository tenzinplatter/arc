use std::fmt;

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::api::ApiClient;

#[derive(
    Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct WorkflowStateId(i64);

impl WorkflowStateId {
    pub fn get(self) -> i64 {
        self.0
    }
}

impl From<i64> for WorkflowStateId {
    fn from(id: i64) -> Self {
        Self(id)
    }
}

impl fmt::Display for WorkflowStateId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct WorkflowState {
    pub id: WorkflowStateId,
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
