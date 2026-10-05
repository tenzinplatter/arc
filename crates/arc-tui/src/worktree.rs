use std::{
    collections::BTreeSet,
    io::Write,
    path::Path,
    process::{Command as StdCommand, Stdio},
};

use anyhow::Context;
use slugify::slugify;
use tokio::process::Command as TokioCommand;
use which::which;

use arc_core::{Config, dbg_file};

pub fn check_worktree_dependencies() -> anyhow::Result<()> {
    which("fd").context("Please make sure 'fd' is installed and in $PATH")?;
    which("fzf").context("Please make sure 'fzf' is installed and in $PATH")?;
    Ok(())
}

async fn find_standard_repos(repos_dir: &str) -> anyhow::Result<Vec<String>> {
    let output = TokioCommand::new("fd")
        .args(["-t", "d", "-I", "-H", "^\\.git$", repos_dir, "-x", "dirname", "{}"])
        .output()
        .await?;

    if !output.status.success() {
        anyhow::bail!("fd failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    let prefix = format!("{}/", repos_dir);
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|l| l.replace(&prefix, ""))
        .filter(|l| !l.is_empty())
        .collect())
}

async fn find_submodule_repos(repos_dir: &str) -> anyhow::Result<Vec<String>> {
    let output = TokioCommand::new("fd")
        .args(["-t", "f", "-I", "-H", "^\\.git$", repos_dir])
        .output()
        .await?;

    if !output.status.success() {
        anyhow::bail!("fd failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    let prefix = format!("{}/", repos_dir);
    let git_files: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|l| l.to_string())
        .collect();

    let contents: Vec<Option<(String, String)>> = futures::future::join_all(
        git_files.into_iter().map(|git_file| async move {
            let content = tokio::fs::read_to_string(&git_file).await.ok()?;
            Some((git_file, content))
        })
    ).await;

    Ok(contents
        .into_iter()
        .flatten()
        .filter(|(_, gitfile_target)| !is_worktree_gitfile(gitfile_target))
        .map(|(git_file, _)| {
            Path::new(&git_file)
                .parent()
                .and_then(|p| p.to_str())
                .unwrap_or("")
                .replace(&prefix, "")
        })
        .filter(|p| !p.is_empty())
        .collect())
}

fn is_worktree_gitfile(gitfile_target: &str) -> bool {
    gitfile_target.contains("worktrees")
}

pub async fn get_repo_list(config: &Config) -> anyhow::Result<String> {
    let repos_dir = config.repositories_directory.to_str().unwrap();

    let (standard, submodules) = tokio::try_join!(
        find_standard_repos(repos_dir),
        find_submodule_repos(repos_dir),
    )?;

    let repos: BTreeSet<String> = standard.into_iter().chain(submodules).collect();
    Ok(repos.into_iter().collect::<Vec<_>>().join("\n"))
}

pub fn select_repo_with_fzf(repo_list: &str) -> anyhow::Result<String> {
    let mut fzf = StdCommand::new("fzf")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .context("failed to spawn fzf")?;

    fzf.stdin
        .take()
        .context("failed to open stdin")?
        .write_all(repo_list.as_bytes())?;

    let output = fzf.wait_with_output()?;

    if !output.status.success() {
        anyhow::bail!("fzf was cancelled or failed");
    }

    Ok(String::from_utf8(output.stdout)
        .context("fzf output was not valid UTF-8")?
        .trim()
        .to_string())
}

pub async fn create_worktree(repo_path: &Path, branch_name: &str) -> anyhow::Result<()> {
    let slug = slugify!(branch_name);
    let worktree_path = format!(".worktrees/{}", slug);

    dbg_file!("repo path {}", repo_path.display());
    TokioCommand::new("mkdir")
        .args(["-p", ".worktrees"])
        .current_dir(repo_path)
        .status()
        .await?;

    if !branch_exists(repo_path, branch_name).await {
        create_branch(repo_path, branch_name).await?;
    }

    let output = TokioCommand::new("git")
        .args(["worktree", "add", &worktree_path, branch_name])
        .current_dir(repo_path)
        .output()
        .await
        .with_context(|| {
            format!(
                "failed to run git worktree add {} {}",
                worktree_path, branch_name
            )
        })?;

    if !output.status.success() {
        anyhow::bail!(
            "git worktree add failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(())
}

async fn branch_exists(repo_path: &Path, branch_name: &str) -> bool {
    let result = TokioCommand::new("git")
        .args([
            "rev-parse",
            "--verify",
            &format!("refs/heads/{}", branch_name),
        ])
        .current_dir(repo_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;

    result.map(|s| s.success()).unwrap_or(false)
}

async fn create_branch(repo_path: &Path, branch_name: &str) -> anyhow::Result<()> {
    let status = TokioCommand::new("git")
        .args(["branch", branch_name])
        .current_dir(repo_path)
        .status()
        .await;

    let success = status.map(|s| s.success()).unwrap_or(false);

    if success {
        Ok(())
    } else {
        anyhow::bail!(
            "Failed to create branch {} for repo at {}",
            branch_name,
            repo_path.display()
        )
    }
}
