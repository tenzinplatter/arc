use std::io::ErrorKind;
use std::path::Path;
use std::process::{Command, Output};

use anyhow::{Context, bail};
use arc_notes::{BranchName, RepoKey};

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

pub fn repo_key(dir: &Path) -> anyhow::Result<Option<RepoKey>> {
    let origin = match git(dir, &["remote", "get-url", "origin"]) {
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        out => out.context("failed to run git")?,
    };
    if !origin.status.success() {
        return Ok(None);
    }
    let url = String::from_utf8(origin.stdout).context("git remote is not UTF-8")?;
    Ok(RepoKey::canonical(&url))
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn run(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "init.defaultBranch=main"])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    fn repo(parent: &Path, name: &str) -> PathBuf {
        let dir = parent.join(name);
        std::fs::create_dir(&dir).unwrap();
        run(&dir, &["init", "--quiet"]);
        dir
    }

    #[test]
    fn a_repo_and_its_worktrees_share_their_origin_key() {
        let tmp = tempfile::tempdir().unwrap();
        let main = repo(tmp.path(), "arc");
        run(&main, &["remote", "add", "origin", "git@github.com:O/Arc.git"]);
        run(&main, &["commit", "--quiet", "--allow-empty", "-m", "root"]);
        let worktree = tmp.path().join("wt");
        run(&main, &["worktree", "add", "--quiet", worktree.to_str().unwrap()]);

        let want = RepoKey::canonical("github.com/o/arc");
        assert_eq!(repo_key(&main).unwrap(), want);
        assert_eq!(repo_key(&worktree).unwrap(), want);
        assert_eq!(repo_key(&main.join(".git")).unwrap(), want);
    }

    #[test]
    fn only_origin_names_the_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let main = repo(tmp.path(), "hearth");
        run(&main, &["remote", "add", "upstream", "git@github.com:o/hearth.git"]);
        assert_eq!(repo_key(&main).unwrap(), None);
    }

    #[test]
    fn a_local_origin_gives_no_key() {
        let tmp = tempfile::tempdir().unwrap();
        let upstream = repo(tmp.path(), "upstream");
        let main = repo(tmp.path(), "hearth");
        run(&main, &["remote", "add", "origin", upstream.to_str().unwrap()]);
        assert_eq!(repo_key(&main).unwrap(), None);
        run(&main, &["remote", "set-url", "origin", "../upstream"]);
        assert_eq!(repo_key(&main).unwrap(), None);
    }

    #[test]
    fn outside_a_repo_there_is_no_key() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(repo_key(tmp.path()).unwrap(), None);
    }
}
