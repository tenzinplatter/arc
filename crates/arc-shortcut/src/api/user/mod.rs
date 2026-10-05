use std::fmt;

use anyhow::{Context, bail};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    api::{ApiClient, get_full_path},
    cache::Cache,
};

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MemberId(Uuid);

impl MemberId {
    pub fn get(self) -> Uuid {
        self.0
    }
}

impl From<Uuid> for MemberId {
    fn from(id: Uuid) -> Self {
        Self(id)
    }
}

impl fmt::Display for MemberId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(try_from = "String")]
pub struct MentionName(String);

impl MentionName {
    pub fn parse(name: &str) -> anyhow::Result<Self> {
        if name.is_empty() {
            bail!("empty mention name");
        }
        Ok(Self(name.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for MentionName {
    type Error = anyhow::Error;

    fn try_from(name: String) -> anyhow::Result<Self> {
        Self::parse(&name)
    }
}

impl fmt::Display for MentionName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct CurrentUser {
    pub id: MemberId,
    pub mention_name: MentionName,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct MemberSlim {
    pub id: MemberId,
    pub mention_name: String,
    pub name: String,
}

#[derive(Deserialize)]
struct RawMember {
    id: MemberId,
    profile: RawProfile,
}

#[derive(Deserialize)]
struct RawProfile {
    mention_name: String,
    #[serde(default)]
    name: Option<String>,
}

impl From<RawMember> for MemberSlim {
    fn from(m: RawMember) -> Self {
        Self {
            id: m.id,
            mention_name: m.profile.mention_name,
            name: m.profile.name.unwrap_or_default(),
        }
    }
}

async fn fetch_current_user(api_token: &str) -> anyhow::Result<CurrentUser> {
    let full_path = get_full_path("member");
    let response = Client::new()
        .get(&full_path)
        .header("Shortcut-Token", api_token)
        .header("Content-Type", "application/json")
        .send()
        .await
        .with_context(|| format!("Failed to GET {}", full_path))?;

    Ok(response.json::<CurrentUser>().await?)
}

impl ApiClient {
    pub async fn from_token_and_cache(api_token: &str, cache: &mut Cache) -> anyhow::Result<Self> {
        if let Some(user) = cache.user() {
            return Ok(Self::new(api_token.to_owned(), user));
        }

        let user = fetch_current_user(api_token).await?;
        cache.set_user(user.clone());
        cache.write().await?;

        Ok(Self::new(api_token.to_owned(), user))
    }

    pub async fn get_members(&self) -> anyhow::Result<Vec<MemberSlim>> {
        let response = self.get("members").await?.error_for_status()?;
        let members: Vec<RawMember> = response
            .json()
            .await
            .context("Failed to parse members response")?;
        Ok(members.into_iter().map(MemberSlim::from).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn member_slim_from_members_payload() {
        let json = r#"[{
            "id": "12345678-1234-1234-1234-123456789abc",
            "disabled": false,
            "role": "member",
            "profile": {"mention_name": "tenzin", "name": "Tenzin P", "email_address": null}
        }]"#;
        let members: Vec<MemberSlim> = serde_json::from_str::<Vec<RawMember>>(json)
            .unwrap()
            .into_iter()
            .map(MemberSlim::from)
            .collect();
        assert_eq!(members[0].mention_name, "tenzin");
        assert_eq!(members[0].name, "Tenzin P");
    }

    #[test]
    fn current_user_needs_a_mention_name() {
        let user: CurrentUser = serde_json::from_str(
            r#"{"id": "12345678-1234-1234-1234-123456789abc", "mention_name": "tenzin"}"#,
        )
        .unwrap();
        assert_eq!(user.mention_name.as_str(), "tenzin");
        assert!(
            serde_json::from_str::<CurrentUser>(
                r#"{"id": "12345678-1234-1234-1234-123456789abc", "mention_name": ""}"#
            )
            .is_err()
        );
        assert!(MentionName::parse("").is_err());
    }
}
