use std::path::Path;
use std::process::{Command, Output};

use anyhow::{Context, bail};
use arc_notes::BranchName;

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

fn git(dir: &Path, args: &[&str]) -> anyhow::Result<Output> {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .context("failed to run git")
}

pub fn head(dir: &Path) -> anyhow::Result<GitHead> {
    let out = git(dir, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
    if out.status.success() {
        let name = String::from_utf8(out.stdout).context("branch name is not UTF-8")?;
        return Ok(GitHead::Branch(BranchName::parse(name.trim_end())?));
    }
    let inside = git(dir, &["rev-parse", "--git-dir"])?;
    Ok(if inside.status.success() {
        GitHead::Detached
    } else {
        GitHead::NotARepo
    })
}

pub fn rename_current_branch(dir: &Path, to: &BranchName) -> anyhow::Result<()> {
    let out = git(dir, &["branch", "-m", to.as_str()])?;
    if !out.status.success() {
        bail!(
            "git branch -m {to} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}
