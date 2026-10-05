use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use anyhow::Context;
use arc_context::OneLine;
use arc_notes::BranchName;
use chrono::{DateTime, FixedOffset, TimeDelta, Utc};

const HEADS: &str = "refs/heads/";
const REMOTES: &str = "refs/remotes/";
const CHECKOUT_PREFIX: &str = "checkout: moving from ";
const CREATED_PREFIX: &str = "branch: Created from ";
const HEAD_REFLOG_SCAN: usize = 200;
pub const FRESH_BRANCH_WINDOW: TimeDelta = TimeDelta::minutes(10);

fn git(dir: &Path, args: &[&str]) -> anyhow::Result<Option<String>> {
    let out = Command::new("git")
        .arg("--no-optional-locks")
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReflogEntry {
    pub at: DateTime<Utc>,
    pub message: String,
}

impl ReflogEntry {
    pub fn parse(line: &str) -> Option<Self> {
        let (selector, message) = line.split_once('\t')?;
        let secs = selector
            .rsplit_once("@{")?
            .1
            .strip_suffix('}')?
            .parse()
            .ok()?;
        Some(Self {
            at: DateTime::from_timestamp(secs, 0)?,
            message: message.to_owned(),
        })
    }

    fn checkout(&self) -> Option<(&str, &str)> {
        self.message
            .strip_prefix(CHECKOUT_PREFIX)?
            .rsplit_once(" to ")
    }
}

fn reflog(dir: &Path, args: &[&str]) -> anyhow::Result<Vec<ReflogEntry>> {
    let mut command = vec!["reflog", "show", "--date=unix", "--format=%gd%x09%gs"];
    command.extend(args);
    Ok(lines(git(dir, &command)?)
        .iter()
        .filter_map(|line| ReflogEntry::parse(line))
        .collect())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StartPoint {
    Head,
    Named(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Created {
    pub at: DateTime<Utc>,
    pub from: StartPoint,
}

impl Created {
    pub fn parse(entry: &ReflogEntry) -> Option<Self> {
        let from = entry.message.strip_prefix(CREATED_PREFIX)?.trim();
        Some(Self {
            at: entry.at,
            from: match from {
                "" => return None,
                "HEAD" | "@" => StartPoint::Head,
                named => StartPoint::Named(named.to_owned()),
            },
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Freshness {
    Fresh(Created),
    Stale,
    Missing,
}

pub fn freshness(oldest_entry: Option<&ReflogEntry>, now: DateTime<FixedOffset>) -> Freshness {
    match oldest_entry.and_then(Created::parse) {
        Some(created) if now.to_utc() - created.at <= FRESH_BRANCH_WINDOW => {
            Freshness::Fresh(created)
        }
        Some(_) | None => Freshness::Stale,
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
    Ok(freshness(reflog(dir, &[&full])?.last(), now))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HeadAt {
    Branch(String),
    Current,
    Unknown,
}

pub fn head_at(
    newest_first: &[ReflogEntry],
    complete: bool,
    new: &BranchName,
    at: DateTime<Utc>,
) -> HeadAt {
    let mut left_at_creation = None;
    for entry in newest_first {
        let Some((from, to)) = entry.checkout() else {
            continue;
        };
        match entry.at.cmp(&at) {
            Ordering::Greater => left_at_creation = Some(from),
            Ordering::Equal if to == new.as_str() => return HeadAt::Branch(from.to_owned()),
            Ordering::Equal if from == new.as_str() => {}
            Ordering::Equal => return HeadAt::Unknown,
            Ordering::Less if to == new.as_str() => return HeadAt::Unknown,
            Ordering::Less => return HeadAt::Branch(to.to_owned()),
        }
    }
    match (complete, left_at_creation) {
        (false, _) => HeadAt::Unknown,
        (true, Some(from)) => HeadAt::Branch(from.to_owned()),
        (true, None) => HeadAt::Current,
    }
}

pub fn head_when_created(
    dir: &Path,
    new: &BranchName,
    at: DateTime<Utc>,
) -> anyhow::Result<HeadAt> {
    let entries = reflog(dir, &[&format!("-n{HEAD_REFLOG_SCAN}"), "HEAD"])?;
    let complete = entries.len() < HEAD_REFLOG_SCAN;
    Ok(head_at(&entries, complete, new, at))
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

    fn entry(at: i64, message: &str) -> ReflogEntry {
        ReflogEntry {
            at: DateTime::from_timestamp(at, 0).unwrap(),
            message: message.to_owned(),
        }
    }

    #[test]
    fn freshness_needs_a_recent_creation_entry() {
        let now = DateTime::parse_from_rfc3339("2026-10-05T10:00:00+10:00").unwrap();
        let secs = now.timestamp();
        let created = |offset: i64, msg: &str| entry(secs - offset, msg);
        assert_eq!(
            freshness(Some(&created(30, "branch: Created from HEAD")), now),
            Freshness::Fresh(Created {
                at: DateTime::from_timestamp(secs - 30, 0).unwrap(),
                from: StartPoint::Head
            })
        );
        assert!(matches!(
            freshness(Some(&created(30, "branch: Created from origin/main")), now),
            Freshness::Fresh(Created { from: StartPoint::Named(n), .. }) if n == "origin/main"
        ));
        assert_eq!(
            freshness(Some(&created(3600, "branch: Created from HEAD")), now),
            Freshness::Stale
        );
        assert_eq!(
            freshness(Some(&created(5, "branch: Reset to dev")), now),
            Freshness::Stale
        );
        assert_eq!(freshness(None, now), Freshness::Stale);
        assert_eq!(ReflogEntry::parse("garbage"), None);
        assert_eq!(
            ReflogEntry::parse("refs/heads/x@{1700000000}\tbranch: Created from HEAD"),
            Some(entry(1_700_000_000, "branch: Created from HEAD"))
        );
    }

    #[test]
    fn head_at_creation_comes_from_the_reflog_around_it() {
        let new = br("me/new");
        let at = DateTime::from_timestamp(100, 0).unwrap();
        let head = |entries: &[ReflogEntry]| head_at(entries, true, &new, at);
        let branch = |b: &str| HeadAt::Branch(b.to_owned());
        assert_eq!(
            head(&[
                entry(100, "checkout: moving from sc-7 to me/new"),
                entry(90, "checkout: moving from main to sc-7"),
            ]),
            branch("sc-7")
        );
        assert_eq!(
            head(&[
                entry(160, "checkout: moving from sc-7 to me/sc-9/y"),
                entry(120, "commit: x"),
                entry(90, "checkout: moving from main to sc-7"),
            ]),
            branch("sc-7")
        );
        assert_eq!(
            head(&[
                entry(160, "checkout: moving from sc-7 to me/sc-9/y"),
                entry(50, "commit (initial): init"),
            ]),
            branch("sc-7")
        );
        assert_eq!(
            head(&[
                entry(100, "checkout: moving from sc-7 to me/sc-9/y"),
                entry(90, "checkout: moving from main to sc-7"),
            ]),
            HeadAt::Unknown
        );
        assert_eq!(
            head(&[
                entry(100, "checkout: moving from me/new to main"),
                entry(100, "checkout: moving from sc-7 to me/new"),
            ]),
            branch("sc-7")
        );
        assert_eq!(head(&[entry(50, "commit (initial): x")]), HeadAt::Current);
        assert_eq!(head(&[]), HeadAt::Current);
        assert_eq!(
            head_at(
                &[entry(160, "checkout: moving from a to b")],
                false,
                &new,
                at
            ),
            HeadAt::Unknown
        );
        assert_eq!(
            head(&[entry(50, "checkout: moving from main to me/new")]),
            HeadAt::Unknown
        );
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
