use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::bail;
use arc_context::{OneLine, SessionTime, activity_since};
use chrono::{DateTime, FixedOffset, Utc};
use serde::{Deserialize, Serialize};

use crate::repo::{git, lines};

const SHA_LENGTHS: [usize; 2] = [40, 64];
const UNTRACKED: &str = "??";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Sha(String);

impl Sha {
    pub fn parse(sha: &str) -> anyhow::Result<Self> {
        if !SHA_LENGTHS.contains(&sha.len()) || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
            bail!("'{sha}' is not a commit id");
        }
        Ok(Self(sha.to_ascii_lowercase()))
    }
}

impl TryFrom<String> for Sha {
    type Error = anyhow::Error;

    fn try_from(sha: String) -> anyhow::Result<Self> {
        Self::parse(&sha)
    }
}

impl From<Sha> for String {
    fn from(sha: Sha) -> Self {
        sha.0
    }
}

impl fmt::Display for Sha {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct StatusCode(String);

impl StatusCode {
    pub fn parse(code: &str) -> anyhow::Result<Self> {
        if code.len() != 2 || !code.is_ascii() {
            bail!("'{code}' is not a git status code");
        }
        Ok(Self(code.to_owned()))
    }

    fn untracked(&self) -> bool {
        self.0 == UNTRACKED
    }

    fn renamed_or_copied(&self) -> bool {
        self.0.contains(['R', 'C'])
    }
}

impl TryFrom<String> for StatusCode {
    type Error = anyhow::Error;

    fn try_from(code: String) -> anyhow::Result<Self> {
        Self::parse(&code)
    }
}

impl From<StatusCode> for String {
    fn from(code: StatusCode) -> Self {
        code.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Change {
    Modified(DateTime<FixedOffset>),
    Deleted(DateTime<FixedOffset>),
}

impl Change {
    fn at(self) -> DateTime<FixedOffset> {
        match self {
            Change::Modified(at) | Change::Deleted(at) => at,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirtyEntry {
    pub status: StatusCode,
    pub path: String,
    pub change: Change,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub root: PathBuf,
    pub head: Option<Sha>,
    pub dirty: Vec<DirtyEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub committer: String,
    pub at: DateTime<FixedOffset>,
    pub subject: OneLine,
}

impl Commit {
    fn parse(line: &str) -> Option<Self> {
        let mut fields = line.splitn(3, '\t');
        let committer = fields.next()?.to_owned();
        let at = DateTime::parse_from_rfc3339(fields.next()?).ok()?;
        let subject = OneLine::parse(fields.next()?)?;
        Some(Self {
            committer,
            at,
            subject,
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionWork {
    pub commits: Vec<Commit>,
    pub changes: Vec<DateTime<FixedOffset>>,
}

impl SessionWork {
    pub fn since(self, baseline: SessionTime) -> Self {
        Self {
            commits: self
                .commits
                .into_iter()
                .filter(|c| activity_since(baseline, c.at))
                .collect(),
            changes: self
                .changes
                .into_iter()
                .filter(|&at| activity_since(baseline, at))
                .collect(),
        }
    }

    pub fn latest(&self) -> Option<DateTime<FixedOffset>> {
        self.commits
            .iter()
            .map(|c| c.at)
            .chain(self.changes.iter().copied())
            .max()
    }

    pub fn is_empty(&self) -> bool {
        self.commits.is_empty() && self.changes.is_empty()
    }

    pub fn subjects(self) -> Vec<OneLine> {
        self.commits.into_iter().map(|c| c.subject).collect()
    }
}

pub fn session_work(
    start: &Snapshot,
    dirty: &[DirtyEntry],
    commits: Vec<Commit>,
    me: Option<&str>,
) -> SessionWork {
    let before: HashMap<&str, &DirtyEntry> =
        start.dirty.iter().map(|e| (e.path.as_str(), e)).collect();
    let changes = dirty
        .iter()
        .filter(|now| match before.get(now.path.as_str()) {
            None => true,
            Some(was) if was.status != now.status => true,
            Some(_) if matches!(now.change, Change::Deleted(_)) => false,
            Some(was) => was.change != now.change,
        })
        .map(|e| e.change.at())
        .collect();
    let commits = commits
        .into_iter()
        .filter(|c| me.is_none_or(|me| c.committer.eq_ignore_ascii_case(me)))
        .collect();
    SessionWork { commits, changes }
}

struct StatusLine {
    status: StatusCode,
    path: String,
}

fn status_lines(porcelain: &str) -> Vec<StatusLine> {
    let mut fields = porcelain.split('\0').filter(|f| !f.is_empty());
    let mut entries = Vec::new();
    while let Some(field) = fields.next() {
        let Some((code, path)) = field.split_at_checked(3) else {
            continue;
        };
        let Ok(status) = StatusCode::parse(&code[..2]) else {
            continue;
        };
        if status.renamed_or_copied() {
            fields.next();
        }
        entries.push(StatusLine {
            status,
            path: path.to_owned(),
        });
    }
    entries
}

fn mtime(path: &Path, now: DateTime<FixedOffset>) -> Option<DateTime<FixedOffset>> {
    std::fs::symlink_metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .map(|at: SystemTime| DateTime::<Utc>::from(at).with_timezone(&now.timezone()))
}

fn change(root: &Path, path: &str, now: DateTime<FixedOffset>) -> Change {
    let full = root.join(path);
    match mtime(&full, now) {
        Some(at) => Change::Modified(at),
        None => Change::Deleted(full.parent().and_then(|dir| mtime(dir, now)).unwrap_or(now)),
    }
}

fn toplevel(dir: &Path) -> anyhow::Result<Option<PathBuf>> {
    Ok(git(dir, &["rev-parse", "--show-toplevel"])?.map(|out| PathBuf::from(out.trim_end())))
}

fn porcelain(root: &Path) -> anyhow::Result<Option<Vec<StatusLine>>> {
    Ok(git(root, &["status", "--porcelain=v1", "-z"])?.map(|out| status_lines(&out)))
}

fn dirty(root: &Path, now: DateTime<FixedOffset>) -> anyhow::Result<Option<Vec<DirtyEntry>>> {
    Ok(porcelain(root)?.map(|lines| {
        lines
            .into_iter()
            .map(|line| DirtyEntry {
                change: change(root, &line.path, now),
                status: line.status,
                path: line.path,
            })
            .collect()
    }))
}

pub fn snapshot(dir: &Path, now: DateTime<FixedOffset>) -> anyhow::Result<Option<Snapshot>> {
    let Some(root) = toplevel(dir)? else {
        return Ok(None);
    };
    let head = git(&root, &["rev-parse", "--verify", "--quiet", "HEAD"])?
        .map(|out| Sha::parse(out.trim()))
        .transpose()?;
    Ok(dirty(&root, now)?.map(|dirty| Snapshot { root, head, dirty }))
}

fn commits_after(root: &Path, start: Option<&Sha>) -> anyhow::Result<Vec<Commit>> {
    let range = match start {
        Some(sha) => format!("{sha}..HEAD"),
        None => "HEAD".to_owned(),
    };
    let log = git(
        root,
        &[
            "log",
            "--no-merges",
            "--format=%ce%x09%cI%x09%s",
            &range,
            "--",
        ],
    )?;
    Ok(lines(log).iter().filter_map(|l| Commit::parse(l)).collect())
}

pub fn work_since(
    dir: &Path,
    start: &Snapshot,
    now: DateTime<FixedOffset>,
) -> anyhow::Result<Option<SessionWork>> {
    if toplevel(dir)?.as_deref() != Some(start.root.as_path()) {
        return Ok(None);
    }
    let Some(dirty) = dirty(&start.root, now)? else {
        return Ok(None);
    };
    let commits = commits_after(&start.root, start.head.as_ref())?;
    let me = git(&start.root, &["config", "user.email"])?;
    Ok(Some(session_work(
        start,
        &dirty,
        commits,
        me.as_deref().map(str::trim),
    )))
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

pub fn uncommitted(dir: &Path) -> anyhow::Result<Option<OneLine>> {
    let Some(root) = toplevel(dir)? else {
        return Ok(None);
    };
    let Some(lines) = porcelain(&root)? else {
        return Ok(None);
    };
    if lines.is_empty() {
        return Ok(None);
    }
    let shortstat = match git(&root, &["diff", "HEAD", "--shortstat"])? {
        Some(stat) => stat,
        None => git(&root, &["diff", "--cached", "--shortstat"])?.unwrap_or_default(),
    };
    let untracked = lines.iter().filter(|l| l.status.untracked()).count();
    Ok(summary(&shortstat, untracked))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(s).unwrap()
    }

    fn entry(status: &str, path: &str, change: Change) -> DirtyEntry {
        DirtyEntry {
            status: StatusCode::parse(status).unwrap(),
            path: path.to_owned(),
            change,
        }
    }

    fn edited(path: &str, at: &str) -> DirtyEntry {
        entry(" M", path, Change::Modified(t(at)))
    }

    fn deleted(path: &str, dir_at: &str) -> DirtyEntry {
        entry(" D", path, Change::Deleted(t(dir_at)))
    }

    fn commit(committer: &str, at: &str, subject: &str) -> Commit {
        Commit {
            committer: committer.to_owned(),
            at: t(at),
            subject: OneLine::parse(subject).unwrap(),
        }
    }

    const BEFORE: &str = "2026-10-05T08:00:00+10:00";
    const DURING: &str = "2026-10-05T09:30:00+10:00";

    fn start(dirty: Vec<DirtyEntry>) -> Snapshot {
        Snapshot {
            root: "/work/repo".into(),
            head: Some(Sha::parse(&"a".repeat(40)).unwrap()),
            dirty,
        }
    }

    #[test]
    fn session_work_table() {
        let me = Some("me@x");
        let cases = vec![
            ("clean and idle", start(vec![]), vec![], vec![], 0, 0),
            (
                "new commit",
                start(vec![]),
                vec![],
                vec![commit("me@x", DURING, "feat: x")],
                1,
                0,
            ),
            (
                "pulled teammates' commits",
                start(vec![]),
                vec![],
                vec![
                    commit("mate@x", DURING, "feat: theirs"),
                    commit("noreply@github.com", DURING, "Merge pull request #1"),
                ],
                0,
                0,
            ),
            (
                "rebased own local commits",
                start(vec![]),
                vec![],
                vec![commit("Me@X", DURING, "feat: mine, replayed")],
                1,
                0,
            ),
            (
                "pre-existing dirty file untouched",
                start(vec![edited("a.rs", BEFORE)]),
                vec![edited("a.rs", BEFORE)],
                vec![],
                0,
                0,
            ),
            (
                "pre-existing dirty file edited again",
                start(vec![edited("a.rs", BEFORE)]),
                vec![edited("a.rs", DURING)],
                vec![],
                0,
                1,
            ),
            (
                "new edit",
                start(vec![edited("a.rs", BEFORE)]),
                vec![edited("a.rs", BEFORE), edited("b.rs", DURING)],
                vec![],
                0,
                1,
            ),
            (
                "pre-existing deletion",
                start(vec![deleted("gone.rs", BEFORE)]),
                vec![deleted("gone.rs", DURING)],
                vec![],
                0,
                0,
            ),
            (
                "new deletion",
                start(vec![]),
                vec![deleted("gone.rs", DURING)],
                vec![],
                0,
                1,
            ),
            (
                "staged after start",
                start(vec![edited("a.rs", BEFORE)]),
                vec![entry("M ", "a.rs", Change::Modified(t(BEFORE)))],
                vec![],
                0,
                1,
            ),
        ];
        for (name, start, dirty, commits, n_commits, n_changes) in cases {
            let work = session_work(&start, &dirty, commits, me);
            assert_eq!(
                (work.commits.len(), work.changes.len()),
                (n_commits, n_changes),
                "{name}"
            );
        }
        let anyone = session_work(
            &start(vec![]),
            &[],
            vec![commit("mate@x", DURING, "x")],
            None,
        );
        assert_eq!(anyone.commits.len(), 1);
    }

    #[test]
    fn work_since_a_baseline() {
        let work = SessionWork {
            commits: vec![
                commit(
                    "me@x",
                    "2026-10-05T10:00:30+10:00",
                    "in the handoff's minute",
                ),
                commit("me@x", "2026-10-05T10:01:00+10:00", "after"),
            ],
            changes: vec![t("2026-10-05T09:00:00+10:00")],
        };
        assert_eq!(work.latest(), Some(t("2026-10-05T10:01:00+10:00")));
        let after = work.since(SessionTime::new(t("2026-10-05T10:00:00+10:00")));
        assert!(after.changes.is_empty());
        assert_eq!(after.subjects(), vec![OneLine::parse("after").unwrap()]);
        assert!(SessionWork::default().is_empty());
        assert_eq!(SessionWork::default().latest(), None);
    }

    #[test]
    fn parses_git_output() {
        let lines = status_lines(" M a.rs\0R  new.rs\0old.rs\0?? dir/\0 D gone\0");
        let parsed: Vec<_> = lines
            .iter()
            .map(|l| (String::from(l.status.clone()), l.path.as_str()))
            .collect();
        assert_eq!(
            parsed,
            vec![
                (" M".into(), "a.rs"),
                ("R ".into(), "new.rs"),
                ("??".into(), "dir/"),
                (" D".into(), "gone"),
            ]
        );
        assert!(lines[2].status.untracked());
        assert_eq!(
            Commit::parse("me@x\t2026-10-05T09:30:00+10:00\tfeat: a\tb"),
            Some(commit("me@x", DURING, "feat: a\tb"))
        );
        assert_eq!(Commit::parse("me@x\tnot a date\tx"), None);
        assert!(Sha::parse(&"A".repeat(40)).is_ok());
        assert!(Sha::parse("abc").is_err());
        assert!(Sha::parse(&"g".repeat(40)).is_err());
        assert!(StatusCode::parse("M").is_err());
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

    #[test]
    fn snapshots_round_trip_through_json() {
        let snap = start(vec![
            edited("a.rs", BEFORE),
            deleted("gone.rs", "2026-10-05T08:00:00.123456789+10:00"),
        ]);
        let json = serde_json::to_string(&snap).unwrap();
        assert_eq!(serde_json::from_str::<Snapshot>(&json).unwrap(), snap);
        assert!(
            json.contains("\"change\":{\"deleted\":\"2026-10-05T08:00:00.123456789+10:00\"}"),
            "{json}"
        );
    }
}
