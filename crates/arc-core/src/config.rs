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

#[derive(Clone, Debug)]
pub struct Config {
    pub notes_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub api_token: String,
    pub editor: String,
    pub repositories_directory: PathBuf,
    pub mux: Mux,
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
