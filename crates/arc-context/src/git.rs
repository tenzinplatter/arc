use std::io::ErrorKind;
use std::path::Path;
use std::process::{Command, Output};

use anyhow::{Context, bail};
use arc_notes::BranchName;

const HEADS: &str = "refs/heads/";
const ORIGIN_HEAD: &str = "refs/remotes/origin/HEAD";
const ORIGIN: &str = "refs/remotes/origin/";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitHead {
    Branch(BranchName),
    Detached,
    NotARepo,
}

impl GitHead {
    pub fn branch(&self) -> Option<&BranchName> {
        match self {
            GitHead::Branch(branch) => Some(branch),
            GitHead::Detached | GitHead::NotARepo => None,
        }
    }

    pub fn require_branch(&self) -> anyhow::Result<&BranchName> {
        match self {
            GitHead::Branch(branch) => Ok(branch),
            GitHead::Detached => bail!("HEAD is detached; check out a branch first"),
            GitHead::NotARepo => bail!("not inside a git repository"),
        }
    }
}

fn git(dir: &Path, args: &[&str]) -> std::io::Result<Output> {
    Command::new("git").arg("-C").arg(dir).args(args).output()
}

fn symbolic_ref(out: Output) -> anyhow::Result<Option<String>> {
    if !out.status.success() {
        return Ok(None);
    }
    let target = String::from_utf8(out.stdout).context("git ref is not UTF-8")?;
    Ok(Some(target.trim_end().to_owned()))
}

pub fn head(dir: &Path) -> anyhow::Result<GitHead> {
    let out = match git(dir, &["symbolic-ref", "--quiet", "HEAD"]) {
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(GitHead::NotARepo),
        out => out.context("failed to run git")?,
    };
    if let Some(target) = symbolic_ref(out)? {
        return Ok(match target.strip_prefix(HEADS) {
            Some(name) => GitHead::Branch(BranchName::parse(name)?),
            None => GitHead::Detached,
        });
    }
    let inside = git(dir, &["rev-parse", "--git-dir"]).context("failed to run git")?;
    Ok(if inside.status.success() {
        GitHead::Detached
    } else {
        GitHead::NotARepo
    })
}

pub fn default_branch(dir: &Path) -> anyhow::Result<Option<BranchName>> {
    let out = git(dir, &["symbolic-ref", "--quiet", ORIGIN_HEAD]).context("failed to run git")?;
    symbolic_ref(out)?
        .as_deref()
        .and_then(|target| target.strip_prefix(ORIGIN))
        .map(BranchName::parse)
        .transpose()
}

pub fn rename_current_branch(dir: &Path, to: &BranchName) -> anyhow::Result<()> {
    let out = git(dir, &["branch", "-m", to.as_str()]).context("failed to run git")?;
    if !out.status.success() {
        bail!(
            "git branch -m {to} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}
