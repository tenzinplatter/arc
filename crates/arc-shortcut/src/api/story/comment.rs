use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Clone, PartialEq, Eq, Debug)]
pub struct StoryComment {
    #[serde(default)]
    pub id: i64,
    pub author_id: Option<crate::api::user::MemberId>,
    #[serde(default)]
    pub created_at: Option<DateTime<Utc>>,
    pub deleted: bool,
    pub position: i32,
    pub text: Option<String>,
}
