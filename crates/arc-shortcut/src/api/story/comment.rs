use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Deserialize, Serialize, Clone, PartialEq, Eq, Debug)]
pub struct StoryComment {
    #[serde(default)]
    pub id: i64,
    pub author_id: Option<Uuid>,
    #[serde(default)]
    pub created_at: Option<DateTime<Utc>>,
    pub deleted: bool,
    // numerical position of comment oldest -> newest
    pub position: i32,
    pub text: Option<String>,
    // TODO: show replies in threads/nested
    // parent_id: Option<i32>
}
