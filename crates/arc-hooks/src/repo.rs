use std::cmp::Ordering;
use std::path::Path;
use std::process::Command;

use anyhow::Context;
use arc_notes::BranchName;
use chrono::{DateTime, FixedOffset, TimeDelta, Utc};

const HEADS: &str = "refs/heads/";
const REMOTES: &str = "refs/remotes/";
const CHECKOUT_PREFIX: &str = "checkout: moving from ";
const CREATED_PREFIX: &str = "branch: Created from ";
const HEAD_REFLOG_SCAN: usize = 200;
pub const FRESH_BRANCH_WINDOW: TimeDelta = TimeDelta::minutes(10);

pub(crate) fn git(dir: &Path, args: &[&str]) -> anyhow::Result<Option<String>> {
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

pub(crate) fn lines(output: Option<String>) -> Vec<String> {
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
}
