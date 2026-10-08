use std::{env, path::PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Mux {
    #[default]
    Tmux,
    Zellij,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
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

#[derive(Clone, Debug)]
pub struct Config {
    pub notes_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub api_token: String,
    pub editor: String,
    pub repositories_directory: PathBuf,
    pub mux: Mux,
    pub notifier: NotifierConfig,
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

        Ok(Config {
            notes_dir,
            cache_dir,
            api_token: config.api_token,
            editor: config.editor,
            repositories_directory,
            mux: config.mux,
            notifier: config.notifier,
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
}
