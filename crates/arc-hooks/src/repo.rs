use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use anyhow::Context;
use arc_context::OneLine;
use arc_notes::BranchName;
use chrono::{DateTime, FixedOffset, TimeDelta};

const HEADS: &str = "refs/heads/";
const REMOTES: &str = "refs/remotes/";
const CHECKOUT_PREFIX: &str = "checkout: moving from ";
const CREATED_PREFIX: &str = "branch: Created from ";
const REFLOG_SCAN: &str = "20";
pub const FRESH_BRANCH_WINDOW: TimeDelta = TimeDelta::minutes(10);

fn git(dir: &Path, args: &[&str]) -> anyhow::Result<Option<String>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .context("failed to run git")?;
    if !out.status.success() {
        return Ok(None);
    }
    Ok(Some(
        String::from_utf8(out.stdout).context("git output is not UTF-8")?,
    ))
}

fn lines(output: Option<String>) -> Vec<String> {
    output
        .map(|out| out.lines().map(str::to_owned).collect())
        .unwrap_or_default()
}

pub fn has_ref(dir: &Path, full: &str) -> anyhow::Result<bool> {
    Ok(git(dir, &["rev-parse", "--verify", "--quiet", full])?.is_some())
}

pub fn local_branch(dir: &Path, name: &str) -> anyhow::Result<Option<BranchName>> {
    let name = name.strip_prefix(HEADS).unwrap_or(name);
    match has_ref(dir, &format!("{HEADS}{name}"))? {
        true => Ok(Some(BranchName::parse(name)?)),
        false => Ok(None),
    }
}

pub fn branch_named_by(dir: &Path, start: &str) -> anyhow::Result<Option<BranchName>> {
    if let Some(branch) = local_branch(dir, start)? {
        return Ok(Some(branch));
    }
    let remote = start.strip_prefix(REMOTES).unwrap_or(start);
    if !has_ref(dir, &format!("{REMOTES}{remote}"))? {
        return Ok(None);
    }
    remote
        .split_once('/')
        .map(|(_, name)| BranchName::parse(name))
        .transpose()
}

pub fn previous_checkout<'a>(reflog: &'a [String], new: &BranchName) -> Option<&'a str> {
    reflog.iter().find_map(|entry| {
        let (from, to) = entry.strip_prefix(CHECKOUT_PREFIX)?.rsplit_once(" to ")?;
        (to == new.as_str()).then_some(from)
    })
}

pub fn branch_before_checkout(dir: &Path, new: &BranchName) -> anyhow::Result<Option<BranchName>> {
    let reflog = lines(git(
        dir,
        &["reflog", "-n", REFLOG_SCAN, "--format=%gs", "HEAD"],
    )?);
    match previous_checkout(&reflog, new) {
        Some(from) => local_branch(dir, from),
        None => Ok(None),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Freshness {
    Fresh,
    Stale,
    Missing,
    Unlogged,
}

pub fn freshness(oldest_entry: Option<&str>, now: DateTime<FixedOffset>) -> Freshness {
    let Some(entry) = oldest_entry else {
        return Freshness::Unlogged;
    };
    let created = entry.split_once('\t').and_then(|(selector, message)| {
        let secs = selector
            .rsplit_once("@{")?
            .1
            .strip_suffix('}')?
            .parse()
            .ok()?;
        message
            .starts_with(CREATED_PREFIX)
            .then(|| DateTime::from_timestamp(secs, 0))
            .flatten()
    });
    match created {
        Some(at) if now.to_utc() - at <= FRESH_BRANCH_WINDOW => Freshness::Fresh,
        _ => Freshness::Stale,
    }
}

pub fn branch_freshness(
    dir: &Path,
    branch: &BranchName,
    now: DateTime<FixedOffset>,
) -> anyhow::Result<Freshness> {
    let full = format!("{HEADS}{branch}");
    if !has_ref(dir, &full)? {
        return Ok(Freshness::Missing);
    }
    let reflog = lines(git(
        dir,
        &[
            "reflog",
            "show",
            "--date=unix",
            "--format=%gd%x09%gs",
            &full,
        ],
    )?);
    Ok(freshness(reflog.last().map(String::as_str), now))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dirty {
    pub newest_change: DateTime<FixedOffset>,
    pub summary: OneLine,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoActivity {
    pub last_commit: Option<DateTime<FixedOffset>>,
    pub dirty: Option<Dirty>,
}

impl RepoActivity {
    pub fn latest(&self) -> Option<DateTime<FixedOffset>> {
        let dirty = self.dirty.as_ref().map(|d| d.newest_change);
        self.last_commit.into_iter().chain(dirty).max()
    }
}

struct StatusEntry {
    path: String,
    untracked: bool,
}

fn status_entries(porcelain: &str) -> Vec<StatusEntry> {
    let mut fields = porcelain.split('\0').filter(|f| !f.is_empty());
    let mut entries = Vec::new();
    while let Some(field) = fields.next() {
        let Some((code, path)) = field.split_at_checked(3) else {
            continue;
        };
        if code.contains(['R', 'C']) {
            fields.next();
        }
        entries.push(StatusEntry {
            path: path.to_owned(),
            untracked: code.starts_with("??"),
        });
    }
    entries
}

fn modified_at(path: &Path, now: DateTime<FixedOffset>) -> DateTime<FixedOffset> {
    std::fs::symlink_metadata(path)
        .and_then(|m| m.modified())
        .map(|at: SystemTime| DateTime::<chrono::Utc>::from(at).with_timezone(&now.timezone()))
        .unwrap_or(now)
}

fn summary(shortstat: &str, untracked: usize) -> Option<OneLine> {
    let untracked = match untracked {
        0 => None,
        1 => Some("1 untracked file".to_owned()),
        n => Some(format!("{n} untracked files")),
    };
    let parts: Vec<String> = [OneLine::parse(shortstat).map(|s| s.to_string()), untracked]
        .into_iter()
        .flatten()
        .collect();
    OneLine::parse(&parts.join("; "))
}

fn dirty(dir: &Path, now: DateTime<FixedOffset>) -> anyhow::Result<Option<Dirty>> {
    let Some(porcelain) = git(dir, &["status", "--porcelain", "-z"])? else {
        return Ok(None);
    };
    let entries = status_entries(&porcelain);
    if entries.is_empty() {
        return Ok(None);
    }
    let root = PathBuf::from(
        git(dir, &["rev-parse", "--show-toplevel"])?
            .context("no work tree")?
            .trim_end(),
    );
    let newest_change = entries
        .iter()
        .map(|e| modified_at(&root.join(&e.path), now))
        .max()
        .unwrap_or(now);
    let shortstat = match git(dir, &["diff", "HEAD", "--shortstat"])? {
        Some(stat) => stat,
        None => git(dir, &["diff", "--cached", "--shortstat"])?.unwrap_or_default(),
    };
    let untracked = entries.iter().filter(|e| e.untracked).count();
    Ok(summary(&shortstat, untracked).map(|summary| Dirty {
        newest_change,
        summary,
    }))
}

pub fn activity(dir: &Path, now: DateTime<FixedOffset>) -> anyhow::Result<RepoActivity> {
    let last_commit = git(dir, &["log", "-1", "--format=%cI", "HEAD"])?
        .map(|out| DateTime::parse_from_rfc3339(out.trim()))
        .transpose()
        .unwrap_or(None);
    Ok(RepoActivity {
        last_commit,
        dirty: dirty(dir, now)?,
    })
}

pub fn commit_subjects_since(
    dir: &Path,
    since: DateTime<FixedOffset>,
) -> anyhow::Result<Vec<OneLine>> {
    let since = format!("--since={}", since.to_rfc3339());
    let log = git(dir, &["log", &since, "--format=%s", "HEAD"])?;
    Ok(lines(log)
        .iter()
        .filter_map(|s| OneLine::parse(s))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn br(s: &str) -> BranchName {
        BranchName::parse(s).unwrap()
    }

    #[test]
    fn previous_checkout_reads_the_newest_matching_move() {
        let reflog: Vec<String> = [
            "checkout: moving from one to two",
            "commit: x",
            "checkout: moving from main to one",
            "checkout: moving from dev to one",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(previous_checkout(&reflog, &br("one")), Some("main"));
        assert_eq!(previous_checkout(&reflog, &br("two")), Some("one"));
        assert_eq!(previous_checkout(&reflog, &br("three")), None);
        let odd = vec!["checkout: moving from a to b to c".to_owned()];
        assert_eq!(previous_checkout(&odd, &br("c")), Some("a to b"));
    }

    #[test]
    fn freshness_needs_a_recent_creation_entry() {
        let now = DateTime::parse_from_rfc3339("2026-10-05T10:00:00+10:00").unwrap();
        let secs = now.timestamp();
        let entry = |offset: i64, msg: &str| format!("x@{{{}}}\t{msg}", secs - offset);
        assert_eq!(
            freshness(Some(&entry(30, "branch: Created from HEAD")), now),
            Freshness::Fresh
        );
        assert_eq!(
            freshness(Some(&entry(3600, "branch: Created from HEAD")), now),
            Freshness::Stale
        );
        assert_eq!(
            freshness(Some(&entry(5, "branch: Reset to dev")), now),
            Freshness::Stale
        );
        assert_eq!(freshness(Some("garbage"), now), Freshness::Stale);
        assert_eq!(freshness(None, now), Freshness::Unlogged);
    }

    #[test]
    fn status_entries_skip_rename_sources() {
        let entries = status_entries(" M a.rs\0R  new.rs\0old.rs\0?? dir/\0");
        let paths: Vec<_> = entries
            .iter()
            .map(|e| (e.path.as_str(), e.untracked))
            .collect();
        assert_eq!(
            paths,
            vec![("a.rs", false), ("new.rs", false), ("dir/", true)]
        );
    }

    #[test]
    fn summaries_combine_diff_and_untracked() {
        let s = |stat: &str, n| summary(stat, n).map(|s| s.to_string());
        assert_eq!(
            s(" 2 files changed, 5 insertions(+)\n", 1).as_deref(),
            Some("2 files changed, 5 insertions(+); 1 untracked file")
        );
        assert_eq!(s("", 3).as_deref(), Some("3 untracked files"));
        assert_eq!(s("", 0), None);
    }
}
