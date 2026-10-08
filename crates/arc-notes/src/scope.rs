use std::fmt;

use anyhow::ensure;
use serde::{Deserialize, Serialize};

use crate::path::NoteRelPath;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RepoKey(String);

impl RepoKey {
    pub fn parse(key: &str) -> anyhow::Result<Self> {
        let key = key.trim().trim_matches('/');
        ensure!(!key.is_empty(), "repo key is empty");
        Ok(Self(key.to_owned()))
    }

    pub fn from_remote(url: &str) -> anyhow::Result<Self> {
        let url = url.trim().trim_end_matches('/');
        let url = url.strip_suffix(".git").unwrap_or(url);
        let (host, path) = match url.split_once("://") {
            Some((_, rest)) => rest.split_once('/').unwrap_or((rest, "")),
            None => url.split_once(':').unwrap_or(("", url)),
        };
        let host = host.rsplit('@').next().unwrap_or(host);
        let host = host.split(':').next().unwrap_or(host).to_lowercase();
        Self::parse(&[host.as_str(), path].join("/"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn todo_file(&self) -> NoteRelPath {
        let segments: Vec<&str> = self.0.split('/').filter(|s| !s.is_empty()).collect();
        let tail = &segments[segments.len().saturating_sub(2)..];
        NoteRelPath::repo_todos(&slugify::slugify(&tail.join(" "), "", "-", None))
    }
}

impl TryFrom<String> for RepoKey {
    type Error = anyhow::Error;

    fn try_from(key: String) -> anyhow::Result<Self> {
        Self::parse(&key)
    }
}

impl From<RepoKey> for String {
    fn from(key: RepoKey) -> Self {
        key.0
    }
}

impl fmt::Display for RepoKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SessionId(String);

impl SessionId {
    pub fn parse(id: &str) -> anyhow::Result<Self> {
        let id = id.trim();
        ensure!(!id.is_empty(), "session id is empty");
        ensure!(
            id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')),
            "session id may only hold letters, digits, `-` and `_`"
        );
        Ok(Self(id.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn token(&self) -> String {
        format!("#session/{}", self.0)
    }
}

impl std::str::FromStr for SessionId {
    type Err = anyhow::Error;

    fn from_str(id: &str) -> anyhow::Result<Self> {
        Self::parse(id)
    }
}

impl TryFrom<String> for SessionId {
    type Error = anyhow::Error;

    fn try_from(id: String) -> anyhow::Result<Self> {
        Self::parse(&id)
    }
}

impl From<SessionId> for String {
    fn from(id: SessionId) -> Self {
        id.0
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoScope {
    #[serde(default)]
    pub repo: Option<RepoKey>,
    #[serde(default)]
    pub session: Option<SessionId>,
}

impl TodoScope {
    pub fn file(&self) -> NoteRelPath {
        self.repo.as_ref().map_or_else(NoteRelPath::todos, RepoKey::todo_file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_remote_spelling_of_a_repo_shares_one_key() {
        for url in [
            "git@github.com:tenzinplatter/arc.git",
            "git@github.com:tenzinplatter/arc",
            "https://github.com/tenzinplatter/arc.git",
            "https://user@GitHub.com/tenzinplatter/arc/",
            "ssh://git@github.com:22/tenzinplatter/arc.git",
        ] {
            assert_eq!(RepoKey::from_remote(url).unwrap().as_str(), "github.com/tenzinplatter/arc", "{url}");
        }
    }

    #[test]
    fn repo_todos_live_in_an_owner_repo_file() {
        let key = RepoKey::from_remote("git@github.com:tenzinplatter/arc.git").unwrap();
        assert_eq!(key.todo_file().as_str(), "todos/tenzinplatter-arc.md");
        assert_eq!(RepoKey::parse("hearth").unwrap().todo_file().as_str(), "todos/hearth.md");
    }

    #[test]
    fn unscoped_todos_go_to_the_global_file() {
        assert_eq!(TodoScope::default().file(), NoteRelPath::todos());
    }

    #[test]
    fn session_ids_reject_what_would_break_the_token() {
        assert!(SessionId::parse("aebef585-bb10-4c7b").is_ok());
        assert!(SessionId::parse("has space").is_err());
        assert!(SessionId::parse("").is_err());
    }
}
