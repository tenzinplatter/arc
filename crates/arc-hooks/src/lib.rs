use std::fmt;
use std::io::{self, Read, Write};
use std::path::PathBuf;

use anyhow::bail;
use arc_core::{Config, dbg_file};
use arc_shortcut::Cache;
use chrono::{DateTime, FixedOffset};

pub mod command;
pub mod input;
pub mod install;
pub mod json;
pub mod marker;
pub mod output;
pub mod post_bash;
pub mod repo;
pub mod session_end;
pub mod session_start;
pub mod shell;
pub mod stop;

use input::HookInput;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hook {
    SessionStart,
    PostBash,
    Stop,
    SessionEnd,
}

impl Hook {
    pub const ALL: [Hook; 4] = [
        Hook::SessionStart,
        Hook::PostBash,
        Hook::Stop,
        Hook::SessionEnd,
    ];

    pub fn subcommand(self) -> &'static str {
        match self {
            Hook::SessionStart => "session-start",
            Hook::PostBash => "post-bash",
            Hook::Stop => "stop",
            Hook::SessionEnd => "session-end",
        }
    }
}

impl fmt::Display for Hook {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.subcommand())
    }
}

pub struct HookEnv {
    pub config: Config,
    pub now: DateTime<FixedOffset>,
    pub home: Option<PathBuf>,
}

impl HookEnv {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            now: arc_core::time::now(),
            home: std::env::home_dir(),
        }
    }
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
) -> anyhow::Result<T> {
    tokio::task::spawn_blocking(work).await?
}

pub async fn respond(
    hook: Hook,
    stdin: &str,
    env: HookEnv,
    cache: Cache,
) -> anyhow::Result<Option<String>> {
    respond_to(hook, HookInput::parse(stdin)?, env, cache).await
}

async fn respond_to(
    hook: Hook,
    input: HookInput,
    env: HookEnv,
    cache: Cache,
) -> anyhow::Result<Option<String>> {
    match (hook, input) {
        (Hook::SessionStart, HookInput::SessionStart(input)) => {
            session_start::run(input, &env, cache).await
        }
        (Hook::PostBash, HookInput::PostToolUse(input)) => {
            blocking(move || post_bash::run(input, &env, &cache)).await
        }
        (Hook::Stop, HookInput::Stop(input)) => blocking(move || stop::run(input, &env)).await,
        (Hook::SessionEnd, HookInput::SessionEnd(input)) => {
            blocking(move || session_end::run(input, &env).map(|_| None)).await
        }
        (hook, _) => bail!("`arc hook {hook}` got input for another hook event"),
    }
}

fn nothing_to_do(hook: Hook, input: &HookInput) -> bool {
    match (hook, input) {
        (Hook::PostBash, HookInput::PostToolUse(input)) => {
            !post_bash::creates_branches(input, std::env::home_dir().as_deref())
        }
        _ => false,
    }
}

async fn respond_to_stdin(hook: Hook, stdin: String) -> anyhow::Result<Option<String>> {
    let input = HookInput::parse(&stdin)?;
    if nothing_to_do(hook, &input) {
        return Ok(None);
    }
    let config = Config::read()?;
    let cache = Cache::read(config.cache_dir.clone())
        .await
        .with_cache_dir(config.cache_dir.clone());
    respond_to(hook, input, HookEnv::new(config), cache).await
}

pub async fn run(hook: Hook) {
    std::panic::set_hook(Box::new(move |panic| dbg_file!("arc hook {hook}: {panic}")));
    let mut stdin = String::new();
    if let Err(e) = io::stdin().read_to_string(&mut stdin) {
        dbg_file!("arc hook {hook}: reading stdin: {e}");
        return;
    }
    let responded = tokio::spawn(respond_to_stdin(hook, stdin))
        .await
        .map_err(anyhow::Error::from)
        .and_then(|responded| responded);
    match responded {
        Ok(Some(out)) => {
            let _ = io::stdout().write_all(out.as_bytes());
        }
        Ok(None) => {}
        Err(e) => dbg_file!("arc hook {hook}: {e:#}"),
    }
}

#[cfg(test)]
mod tests;
