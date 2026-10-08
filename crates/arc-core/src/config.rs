use std::{env, fmt, path::PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Mux {
    #[default]
    Tmux,
    Zellij,
}

#[derive(Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum NotifierConfig {
    #[default]
    Desktop,
    Ntfy {
        url: String,
        topic: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token: Option<String>,
    },
    #[serde(rename = "none")]
    Silent,
}

impl NotifierConfig {
    pub fn is_desktop(&self) -> bool {
        matches!(self, NotifierConfig::Desktop)
    }
}

impl fmt::Debug for NotifierConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NotifierConfig::Desktop => f.write_str("Desktop"),
            NotifierConfig::Ntfy { url, topic, token } => f
                .debug_struct("Ntfy")
                .field("url", url)
                .field("topic", topic)
                .field("token", &token.as_ref().map(|_| "<redacted>"))
                .finish(),
            NotifierConfig::Silent => f.write_str("Silent"),
        }
    }
}

#[derive(Clone)]
pub struct Config {
    pub notes_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub api_token: String,
    pub editor: String,
    pub repositories_directory: PathBuf,
    pub mux: Mux,
    pub notifier: NotifierConfig,
    pub mcp_token: Option<BearerToken>,
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("notes_dir", &self.notes_dir)
            .field("cache_dir", &self.cache_dir)
            .field("api_token", &"<redacted>")
            .field("editor", &self.editor)
            .field("repositories_directory", &self.repositories_directory)
            .field("mux", &self.mux)
            .field("notifier", &self.notifier)
            .field("mcp_token", &self.mcp_token)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct BearerToken(String);

impl BearerToken {
    pub fn parse(token: &str) -> anyhow::Result<Self> {
        if token.is_empty() || token.contains(char::is_whitespace) {
            anyhow::bail!("mcp_token must be non-empty and contain no whitespace");
        }
        Ok(Self(token.to_owned()))
    }

    pub fn matches(&self, presented: &str) -> bool {
        let (expected, presented) = (self.0.as_bytes(), presented.as_bytes());
        expected.len() == presented.len()
            && expected
                .iter()
                .zip(presented)
                .fold(0u8, |diff, (a, b)| diff | (a ^ b))
                == 0
    }

    fn reveal(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for BearerToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BearerToken(<redacted>)")
    }
}

#[derive(Deserialize, Serialize, Clone)]
struct ConfigFile {
    notes_dir: String,
    #[serde(default = "default_cache_dir_string")]
    cache_dir: String,
    api_token: String,
    #[serde(default = "default_editor")]
    editor: String,
    #[serde(default = "default_repositories_directory")]
    repositories_directory: String,
    #[serde(default)]
    mux: Mux,
    #[serde(default, skip_serializing_if = "NotifierConfig::is_desktop")]
    notifier: NotifierConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mcp_token: Option<String>,
}

impl Default for ConfigFile {
    fn default() -> Self {
        Self {
            notes_dir: String::new(),
            cache_dir: default_cache_dir_string(),
            api_token: String::new(),
            editor: default_editor(),
            repositories_directory: default_repositories_directory(),
            mux: Mux::default(),
            notifier: NotifierConfig::default(),
            mcp_token: None,
        }
    }
}

fn default_cache_dir_string() -> String {
    default_cache_dir().to_str().unwrap().to_string()
}

fn default_editor() -> String {
    env::var("EDITOR").unwrap_or_default()
}

fn default_repositories_directory() -> String {
    "~/Repositories".to_string()
}

impl Config {
    pub fn read() -> anyhow::Result<Config> {
        let config: ConfigFile = confy::load("arc", Some("config"))?;

        if config.notes_dir.is_empty() {
            anyhow::bail!("notes_dir is required in config");
        }
        if config.api_token.is_empty() {
            anyhow::bail!("api_token is required in config");
        }
        if config.editor.is_empty() {
            anyhow::bail!("editor not set in config and $EDITOR is not set");
        }

        let notes_dir = expand_tilde(&config.notes_dir)?;
        let cache_dir = expand_tilde(&config.cache_dir)?;
        let repositories_directory = expand_tilde(&config.repositories_directory)?;
        let mcp_token = config
            .mcp_token
            .as_deref()
            .map(BearerToken::parse)
            .transpose()?;

        Ok(Config {
            notes_dir,
            cache_dir,
            api_token: config.api_token,
            editor: config.editor,
            repositories_directory,
            mux: config.mux,
            notifier: config.notifier,
            mcp_token,
        })
    }

    pub fn write(&self) -> anyhow::Result<()> {
        let config = ConfigFile {
            notes_dir: self.notes_dir.to_str().unwrap().to_string(),
            cache_dir: self.cache_dir.to_str().unwrap().to_string(),
            api_token: self.api_token.clone(),
            editor: self.editor.clone(),
            repositories_directory: self.repositories_directory.to_str().unwrap().to_string(),
            mux: self.mux.clone(),
            notifier: self.notifier.clone(),
            mcp_token: self.mcp_token.as_ref().map(|t| t.reveal().to_owned()),
        };

        confy::store("arc", Some("config"), config).context("Failed to write config")
    }
}

fn expand_tilde(path: &str) -> anyhow::Result<PathBuf> {
    let expanded =
        shellexpand::full(path).with_context(|| format!("can't expand config path '{path}'"))?;
    Ok(PathBuf::from(expanded.into_owned()))
}

fn default_cache_dir() -> PathBuf {
    env::home_dir()
        .expect("Couldn't find home dir")
        .join(".cache")
        .join("arc")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(toml_src: &str) -> Result<ConfigFile, toml::de::Error> {
        toml::from_str(toml_src)
    }

    const BASE: &str = "notes_dir = \"~/notes\"\napi_token = \"t\"\n";

    #[test]
    fn notifier_defaults_to_desktop_when_absent() {
        assert_eq!(parse(BASE).unwrap().notifier, NotifierConfig::Desktop);
    }

    #[test]
    fn parses_desktop_notifier() {
        let cfg = parse(&format!("{BASE}[notifier]\nkind = \"desktop\"\n")).unwrap();
        assert_eq!(cfg.notifier, NotifierConfig::Desktop);
    }

    #[test]
    fn parses_none_notifier() {
        let cfg = parse(&format!("{BASE}[notifier]\nkind = \"none\"\n")).unwrap();
        assert_eq!(cfg.notifier, NotifierConfig::Silent);
    }

    #[test]
    fn parses_ntfy_notifier_without_token() {
        let cfg = parse(&format!(
            "{BASE}[notifier]\nkind = \"ntfy\"\nurl = \"http://ntfy.local:2586\"\ntopic = \"arc\"\n"
        ))
        .unwrap();
        assert_eq!(
            cfg.notifier,
            NotifierConfig::Ntfy {
                url: "http://ntfy.local:2586".into(),
                topic: "arc".into(),
                token: None,
            }
        );
    }

    #[test]
    fn parses_ntfy_notifier_with_token() {
        let cfg = parse(&format!(
            "{BASE}[notifier]\nkind = \"ntfy\"\nurl = \"http://ntfy.local\"\ntopic = \"arc\"\ntoken = \"tk_secret\"\n"
        ))
        .unwrap();
        assert_eq!(
            cfg.notifier,
            NotifierConfig::Ntfy {
                url: "http://ntfy.local".into(),
                topic: "arc".into(),
                token: Some("tk_secret".into()),
            }
        );
    }

    #[test]
    fn rejects_ntfy_without_topic() {
        let err = parse(&format!(
            "{BASE}[notifier]\nkind = \"ntfy\"\nurl = \"http://ntfy.local\"\n"
        ))
        .err()
        .expect("ntfy without a topic must not parse");
        assert!(err.to_string().contains("topic"), "{err}");
    }

    #[test]
    fn rejects_unknown_notifier_kind() {
        assert!(parse(&format!("{BASE}[notifier]\nkind = \"pager\"\n")).is_err());
    }

    #[test]
    fn desktop_notifier_writes_no_notifier_table() {
        let written = toml::to_string(&parse(BASE).unwrap()).unwrap();
        assert!(!written.contains("notifier"), "{written}");
        assert_eq!(parse(&written).unwrap().notifier, NotifierConfig::Desktop);
    }

    #[test]
    fn notifier_round_trips_through_toml() {
        let file = ConfigFile {
            notifier: NotifierConfig::Ntfy {
                url: "http://ntfy.local".into(),
                topic: "arc".into(),
                token: None,
            },
            ..parse(BASE).unwrap()
        };
        let written = toml::to_string(&file).unwrap();
        assert_eq!(parse(&written).unwrap().notifier, file.notifier);
    }

    #[test]
    fn bearer_tokens_match_only_the_exact_secret() {
        let token = BearerToken::parse("s3cret").unwrap();
        assert!(token.matches("s3cret"));
        for wrong in ["", "s3cre", "s3cret!", "S3cret", "x3cret"] {
            assert!(!token.matches(wrong), "{wrong}");
        }
        assert_eq!(format!("{token:?}"), "BearerToken(<redacted>)");
    }

    #[test]
    fn config_debug_redacts_secrets() {
        let config = Config {
            notes_dir: PathBuf::from("/n"),
            cache_dir: PathBuf::from("/c"),
            api_token: "shortcut-secret".into(),
            editor: String::new(),
            repositories_directory: PathBuf::from("/r"),
            mux: Mux::default(),
            notifier: NotifierConfig::Ntfy {
                url: "http://ntfy.local".into(),
                topic: "arc".into(),
                token: Some("ntfy-secret".into()),
            },
            mcp_token: Some(BearerToken::parse("mcp-secret").unwrap()),
        };
        let debug = format!("{config:?}");
        for secret in ["shortcut-secret", "mcp-secret", "ntfy-secret"] {
            assert!(!debug.contains(secret), "{debug}");
        }
        assert!(
            debug.contains("\"/n\"") && debug.contains("http://ntfy.local"),
            "{debug}"
        );
    }

    #[test]
    fn bearer_tokens_reject_empty_or_spaced_secrets() {
        for bad in ["", " ", "a b", "tok\n"] {
            assert!(BearerToken::parse(bad).is_err(), "{bad:?}");
        }
    }
}
