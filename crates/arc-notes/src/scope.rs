use std::fmt;

use anyhow::{anyhow, ensure};
use serde::{Deserialize, Deserializer, Serialize};

use crate::path::NoteRelPath;

const REMOTE_SCHEMES: [&str; 6] = ["https", "http", "ssh", "git", "git+ssh", "ssh+git"];
const SESSION_TOKEN_PREFIX: &str = "#session/";

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PathSegment(String);

impl PathSegment {
    fn parse(raw: &str) -> Option<Self> {
        let safe = !raw.is_empty()
            && raw != "."
            && raw != ".."
            && raw.chars().all(|c| matches!(c, 'a'..='z' | '0'..='9' | '.' | '_' | '~' | '-'));
        safe.then(|| Self(raw.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PathSegment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RepoKey {
    host: PathSegment,
    owner: PathSegment,
    subgroups: Vec<PathSegment>,
    name: PathSegment,
}

impl RepoKey {
    pub fn canonical(remote: &str) -> Option<Self> {
        let lowered = remote.trim().to_lowercase();
        let without_query = lowered.split(['?', '#']).next().unwrap_or_default();
        let (authority, path) = split_remote(without_query)?;
        let credentialless = authority.rsplit('@').next().unwrap_or_default();
        let host = PathSegment::parse(credentialless.split(':').next().unwrap_or_default())?;

        let mut segments: Vec<&str> = path.split('/').filter(|segment| !segment.is_empty()).collect();
        let name = segments.pop()?;
        let name = PathSegment::parse(name.strip_suffix(".git").unwrap_or(name))?;
        let mut namespace = segments.into_iter().map(PathSegment::parse);
        let owner = namespace.next()??;
        let subgroups = namespace.collect::<Option<Vec<_>>>()?;
        Some(Self { host, owner, subgroups, name })
    }

    pub fn segments(&self) -> impl Iterator<Item = &PathSegment> {
        [&self.host, &self.owner].into_iter().chain(&self.subgroups).chain([&self.name])
    }
}

fn split_remote(remote: &str) -> Option<(&str, &str)> {
    match remote.split_once("://") {
        Some((scheme, rest)) if REMOTE_SCHEMES.contains(&scheme) => rest.split_once('/'),
        Some(_) => None,
        None => match remote.split_once(':') {
            Some((authority, path)) if !authority.contains('/') => Some((authority, path)),
            _ => remote.split_once('/'),
        },
    }
}

impl TryFrom<String> for RepoKey {
    type Error = anyhow::Error;

    fn try_from(key: String) -> anyhow::Result<Self> {
        Self::canonical(&key).ok_or_else(|| anyhow!("`{key}` names no hosted repo"))
    }
}

impl From<RepoKey> for String {
    fn from(key: RepoKey) -> Self {
        key.to_string()
    }
}

impl fmt::Display for RepoKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.host, self.owner)?;
        self.subgroups.iter().try_for_each(|group| write!(f, "/{group}"))?;
        write!(f, "/{}", self.name)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NoteRepo {
    Key(RepoKey),
    Unrecognised(String),
}

impl NoteRepo {
    pub fn parse(raw: String) -> Self {
        RepoKey::canonical(&raw).map_or(Self::Unrecognised(raw), Self::Key)
    }

    pub fn key(&self) -> Option<&RepoKey> {
        match self {
            Self::Key(key) => Some(key),
            Self::Unrecognised(_) => None,
        }
    }
}

impl From<NoteRepo> for String {
    fn from(repo: NoteRepo) -> Self {
        match repo {
            NoteRepo::Key(key) => key.to_string(),
            NoteRepo::Unrecognised(raw) => raw,
        }
    }
}

impl fmt::Display for NoteRepo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Key(key) => key.fmt(f),
            Self::Unrecognised(raw) => f.write_str(raw),
        }
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

    pub fn from_token(word: &str) -> Option<Self> {
        Self::parse(word.strip_prefix(SESSION_TOKEN_PREFIX)?).ok()
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn token(&self) -> String {
        format!("{SESSION_TOKEN_PREFIX}{}", self.0)
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
    #[serde(default, deserialize_with = "recognised_repo")]
    pub repo: Option<RepoKey>,
    #[serde(default)]
    pub session: Option<SessionId>,
}

fn recognised_repo<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<RepoKey>, D::Error> {
    Ok(Option::<String>::deserialize(deserializer)?.as_deref().and_then(RepoKey::canonical))
}

impl TodoScope {
    pub fn file(&self) -> NoteRelPath {
        self.repo.as_ref().map_or_else(NoteRelPath::todos, NoteRelPath::repo_todos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(remote: &str) -> Option<String> {
        RepoKey::canonical(remote).map(|key| key.to_string())
    }

    #[test]
    fn every_remote_spelling_of_a_repo_shares_one_key() {
        for url in [
            "git@github.com:tenzinplatter/arc.git",
            "git@github.com:tenzinplatter/arc",
            "git@github.com:/tenzinplatter/arc.git",
            "git@GitHub.com:TenzinPlatter/Arc.git",
            "github.com:tenzinplatter/arc",
            "https://github.com/tenzinplatter/arc.git",
            "https://github.com/tenzinplatter/arc.git/",
            "https://github.com/tenzinplatter/arc/",
            "HTTPS://GitHub.com/TenzinPlatter/Arc.git",
            "http://github.com/tenzinplatter/arc",
            "https://github.com:443/tenzinplatter/arc.git",
            "https://github.com//tenzinplatter///arc.git",
            "https://github.com/tenzinplatter/arc.git?ref=main",
            "https://github.com/tenzinplatter/arc#readme",
            "ssh://git@github.com/tenzinplatter/arc.git",
            "ssh://git@github.com:22/tenzinplatter/arc.git",
            "git+ssh://git@github.com/tenzinplatter/arc.git",
            "git://github.com/tenzinplatter/arc.git",
            "github.com/tenzinplatter/arc",
            "  github.com/tenzinplatter/arc\n",
        ] {
            assert_eq!(key(url).as_deref(), Some("github.com/tenzinplatter/arc"), "{url}");
        }
    }

    #[test]
    fn credentials_never_reach_the_key() {
        for url in [
            "https://user@github.com/o/r.git",
            "https://user:ghp_s3cret@github.com/o/r.git",
            "https://x-access-token:ghp_s3cret@github.com:443/o/r",
            "ssh://user:pw@github.com:2222/o/r.git",
            "deploy@github.com:o/r.git",
        ] {
            assert_eq!(key(url).as_deref(), Some("github.com/o/r"), "{url}");
        }
        for leaky in ["https://user:pa/ss@github.com/o/r", "user:pw@github.com:o/r"] {
            assert_eq!(key(leaky), None, "{leaky}");
        }
    }

    #[test]
    fn subgroups_keep_every_segment() {
        assert_eq!(key("git@gitlab.com:g/sub/r.git").as_deref(), Some("gitlab.com/g/sub/r"));
        assert_eq!(key("https://gitlab.com/g/sub/deeper/r").as_deref(), Some("gitlab.com/g/sub/deeper/r"));
    }

    #[test]
    fn remotes_without_a_host_and_owner_have_no_key() {
        for url in [
            "",
            "/srv/git/r.git",
            "../foo",
            "./foo/bar/baz",
            "file:///srv/git/o/r.git",
            "file://host/o/r.git",
            "ftp://host/o/r.git",
            "hearth",
            "github.com/r",
            "git@github.com:r.git",
            "https://github.com/",
            "https://github.com/o/.git",
            "https://github.com/o/../r",
            "https://github.com/o/r é",
            "C:\\repos\\r",
        ] {
            assert_eq!(key(url), None, "{url}");
        }
    }

    #[test]
    fn the_key_parses_back_to_itself() {
        let key = RepoKey::canonical("git@gitlab.com:G/Sub/R.git").unwrap();
        assert_eq!(RepoKey::canonical(&key.to_string()), Some(key));
    }

    #[test]
    fn serde_canonicalises_and_rejects_hostless_keys() {
        let key: RepoKey = serde_json::from_str("\"https://GitHub.com/O/R.git\"").unwrap();
        assert_eq!(serde_json::to_string(&key).unwrap(), "\"github.com/o/r\"");
        assert!(serde_json::from_str::<RepoKey>("\"hearth\"").is_err());
    }

    #[test]
    fn a_cached_scope_with_an_unrecognised_repo_reads_as_global() {
        let scope: TodoScope = serde_json::from_str(r#"{"repo":"hearth","session":null}"#).unwrap();
        assert_eq!(scope, TodoScope::default());
    }

    #[test]
    fn note_repos_keep_what_they_cannot_canonicalise() {
        assert_eq!(String::from(NoteRepo::parse("https://github.com/O/R.git".into())), "github.com/o/r");
        let raw = NoteRepo::parse("my local thing".into());
        assert_eq!(raw.key(), None);
        assert_eq!(String::from(raw), "my local thing");
    }

    #[test]
    fn repo_todos_live_at_the_key_path() {
        let path = |remote: &str| TodoScope { repo: RepoKey::canonical(remote), session: None }.file();
        assert_eq!(path("git@github.com:tenzinplatter/arc.git").as_str(), "todos/github.com/tenzinplatter/arc.md");
        assert_eq!(path("git@gitlab.com:g/sub/r.git").as_str(), "todos/gitlab.com/g/sub/r.md");
    }

    #[test]
    fn distinct_keys_never_share_a_todo_file() {
        let path = |key: &str| NoteRelPath::repo_todos(&RepoKey::canonical(key).unwrap());
        for (a, b) in [
            ("github.com/a-b/c", "github.com/a/b-c"),
            ("github.com/o/my_repo", "github.com/o/my.repo"),
            ("gitlab.com/g/sub/r", "gitlab.com/g/sub-r"),
            ("gitlab.com/g/sub/r", "gitlab.com/g-sub/r"),
        ] {
            assert_ne!(path(a), path(b), "{a} vs {b}");
        }
    }

    #[test]
    fn todo_files_survive_note_path_validation() {
        for remote in ["github.com/o/.github", "gitlab.com/g/sub/r", "github.com/o/r.js"] {
            let file = NoteRelPath::repo_todos(&RepoKey::canonical(remote).unwrap());
            assert_eq!(NoteRelPath::parse(file.as_str()).unwrap(), file, "{remote}");
        }
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
        assert_eq!(SessionId::from_token("#session/ab_1"), SessionId::parse("ab_1").ok());
        assert_eq!(SessionId::from_token("#session/x.y"), None);
        assert_eq!(SessionId::from_token("session/x"), None);
    }
}
