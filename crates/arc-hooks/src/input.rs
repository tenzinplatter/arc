use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::bail;
use serde::{Deserialize, Serialize};

const MAX_SESSION_ID_LEN: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SessionId(String);

impl SessionId {
    pub fn parse(id: &str) -> anyhow::Result<Self> {
        let safe = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '_');
        if id.is_empty() || id.len() > MAX_SESSION_ID_LEN || !id.chars().all(safe) {
            bail!("'{id}' is not a session id");
        }
        Ok(Self(id.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
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

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "PathBuf", into = "PathBuf")]
pub struct Cwd(PathBuf);

impl Cwd {
    pub fn parse(dir: PathBuf) -> anyhow::Result<Self> {
        if !dir.is_absolute() {
            bail!("hook cwd '{}' is not absolute", dir.display());
        }
        Ok(Self(dir))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl TryFrom<PathBuf> for Cwd {
    type Error = anyhow::Error;

    fn try_from(dir: PathBuf) -> anyhow::Result<Self> {
        Self::parse(dir)
    }
}

impl From<Cwd> for PathBuf {
    fn from(cwd: Cwd) -> Self {
        cwd.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StartSource {
    Startup,
    Resume,
    Clear,
    Compact,
    Fork,
    #[serde(other)]
    Other,
}

impl StartSource {
    pub fn continues_session(self) -> bool {
        match self {
            StartSource::Resume | StartSource::Compact => true,
            StartSource::Startup | StartSource::Clear | StartSource::Fork | StartSource::Other => {
                false
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct SessionStartInput {
    pub session_id: SessionId,
    pub cwd: Cwd,
    pub source: StartSource,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct BashInput {
    pub command: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "tool_name")]
pub enum ToolCall {
    Bash {
        tool_input: BashInput,
    },
    #[serde(other)]
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct PostToolUseInput {
    pub session_id: SessionId,
    pub cwd: Cwd,
    #[serde(flatten)]
    pub tool: ToolCall,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct StopInput {
    pub session_id: SessionId,
    pub cwd: Cwd,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct SessionEndInput {
    pub session_id: SessionId,
    pub cwd: Cwd,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "hook_event_name")]
pub enum HookInput {
    SessionStart(SessionStartInput),
    PostToolUse(PostToolUseInput),
    Stop(StopInput),
    SessionEnd(SessionEndInput),
}

impl HookInput {
    pub fn parse(json: &str) -> anyhow::Result<Self> {
        Ok(serde_json::from_str(json)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SESSION_START: &str = r#"{
        "session_id": "abc123",
        "transcript_path": "/Users/x/.claude/projects/p/transcript.jsonl",
        "cwd": "/Users/x/proj",
        "hook_event_name": "SessionStart",
        "source": "resume",
        "model": "claude-opus-5",
        "seconds_since_last_response": 5400
    }"#;

    const POST_BASH: &str = r#"{
        "session_id": "abc123",
        "hook_event_name": "PostToolUse",
        "tool_name": "Bash",
        "tool_input": {
            "command": "git checkout -b feature",
            "description": "Create branch",
            "timeout": 120000,
            "run_in_background": false
        },
        "tool_use_id": "toolu_01ABC123",
        "tool_response": {"stdout": "", "stderr": "Switched to a new branch 'feature'", "interrupted": false},
        "cwd": "/home/user/my-project",
        "permission_mode": "default"
    }"#;

    const STOP: &str = r#"{
        "session_id": "abc123",
        "hook_event_name": "Stop",
        "cwd": "/home/user/my-project",
        "transcript_path": "/home/user/.claude/projects/p/transcript.jsonl",
        "last_assistant_message": "I've completed the refactoring.",
        "permission_mode": "default"
    }"#;

    const SESSION_END: &str = r#"{
        "session_id": "abc123",
        "hook_event_name": "SessionEnd",
        "cwd": "/home/user/my-project",
        "transcript_path": "/home/user/.claude/projects/p/transcript.jsonl",
        "reason": "prompt_input_exit"
    }"#;

    fn id() -> SessionId {
        SessionId::parse("abc123").unwrap()
    }

    fn cwd(dir: &str) -> Cwd {
        Cwd::parse(dir.into()).unwrap()
    }

    #[test]
    fn parses_each_documented_event() {
        assert_eq!(
            HookInput::parse(SESSION_START).unwrap(),
            HookInput::SessionStart(SessionStartInput {
                session_id: id(),
                cwd: cwd("/Users/x/proj"),
                source: StartSource::Resume,
            })
        );
        assert_eq!(
            HookInput::parse(POST_BASH).unwrap(),
            HookInput::PostToolUse(PostToolUseInput {
                session_id: id(),
                cwd: cwd("/home/user/my-project"),
                tool: ToolCall::Bash {
                    tool_input: BashInput {
                        command: "git checkout -b feature".into()
                    }
                },
            })
        );
        assert_eq!(
            HookInput::parse(STOP).unwrap(),
            HookInput::Stop(StopInput {
                session_id: id(),
                cwd: cwd("/home/user/my-project"),
            })
        );
        assert_eq!(
            HookInput::parse(SESSION_END).unwrap(),
            HookInput::SessionEnd(SessionEndInput {
                session_id: id(),
                cwd: cwd("/home/user/my-project"),
            })
        );
    }

    #[test]
    fn other_tools_and_sources_still_parse() {
        let edit = r#"{"session_id":"s","cwd":"/x","hook_event_name":"PostToolUse","tool_name":"Edit","tool_input":{"file_path":"/x/a"}}"#;
        assert!(matches!(
            HookInput::parse(edit).unwrap(),
            HookInput::PostToolUse(PostToolUseInput {
                tool: ToolCall::Other,
                ..
            })
        ));
        let fork =
            r#"{"session_id":"s","cwd":"/x","hook_event_name":"SessionStart","source":"fork"}"#;
        let future =
            r#"{"session_id":"s","cwd":"/x","hook_event_name":"SessionStart","source":"teleport"}"#;
        for (json, source) in [(fork, StartSource::Fork), (future, StartSource::Other)] {
            assert!(matches!(
                HookInput::parse(json).unwrap(),
                HookInput::SessionStart(input) if input.source == source
            ));
        }
    }

    #[test]
    fn rejects_unsafe_ids_relative_cwds_and_unknown_events() {
        let with = |id: &str, cwd: &str, event: &str| {
            HookInput::parse(&format!(
                r#"{{"session_id":{id:?},"cwd":{cwd:?},"hook_event_name":{event:?}}}"#
            ))
        };
        assert!(with("abc-1_2", "/x", "Stop").is_ok());
        assert!(with("../escape", "/x", "Stop").is_err());
        assert!(with("", "/x", "Stop").is_err());
        assert!(with("a/b", "/x", "Stop").is_err());
        assert!(with("abc", "relative", "Stop").is_err());
        assert!(with("abc", "/x", "Notification").is_err());
        assert!(HookInput::parse("not json").is_err());
        let bash_without_command = r#"{"session_id":"s","cwd":"/x","hook_event_name":"PostToolUse","tool_name":"Bash","tool_input":{}}"#;
        assert!(HookInput::parse(bash_without_command).is_err());
    }

    #[test]
    fn resume_and_compact_continue_the_session() {
        assert!(StartSource::Resume.continues_session());
        assert!(StartSource::Compact.continues_session());
        assert!(!StartSource::Startup.continues_session());
        assert!(!StartSource::Clear.continues_session());
        assert!(!StartSource::Fork.continues_session());
    }
}
