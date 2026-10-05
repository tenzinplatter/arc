use std::path::{Path, PathBuf};

use arc_notes::BranchName;

use crate::shell::simple_commands;

const REF_FORBIDDEN: [char; 11] = [' ', '~', '^', ':', '?', '*', '[', '\\', '$', '`', '\''];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Creator {
    Checkout,
    Branch,
    Worktree,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParentRef {
    StartPoint(String),
    PreviousCheckout,
    CurrentHead,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchCreation {
    pub repo: PathBuf,
    pub new: BranchName,
    pub parent: ParentRef,
}

fn ref_name(word: &str) -> Option<BranchName> {
    let plausible = !word.starts_with('-')
        && !word.contains("..")
        && !word.contains("@{")
        && !word.ends_with(['.', '/'])
        && !word
            .chars()
            .any(|c| c.is_control() || REF_FORBIDDEN.contains(&c));
    plausible.then(|| BranchName::parse(word).ok()).flatten()
}

fn start_point(word: Option<&String>, creator: &Creator) -> Option<ParentRef> {
    Some(match word.map(String::as_str) {
        None | Some("HEAD" | "@") => match creator {
            Creator::Checkout => ParentRef::PreviousCheckout,
            Creator::Branch | Creator::Worktree => ParentRef::CurrentHead,
        },
        Some(start) if start.starts_with('-') => return None,
        Some(start) => ParentRef::StartPoint(start.to_owned()),
    })
}

fn short_flag_value<'a>(
    arg: &'a str,
    letters: &[char],
    rest: &mut impl Iterator<Item = &'a String>,
) -> Option<Option<&'a str>> {
    let flags = arg.strip_prefix('-').filter(|f| !f.starts_with('-'))?;
    let at = flags.find(|c| letters.contains(&c))?;
    let attached = &flags[at + 1..];
    Some(match attached {
        "" => rest.next().map(String::as_str),
        name => Some(name),
    })
}

fn switching(
    args: &[String],
    create: &[char],
    long: &[&str],
    repo: &Path,
) -> Option<BranchCreation> {
    let mut rest = args.iter();
    let mut new = None;
    let mut positionals = Vec::new();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--" => break,
            "--orphan" | "--detach" => return None,
            a if a.starts_with("--orphan=") => return None,
            a if long.contains(&a) => new = Some(rest.next()?.as_str()),
            a if long.iter().any(|l| a.starts_with(&format!("{l}="))) => {
                new = a.split_once('=').map(|(_, name)| name)
            }
            a if a.starts_with("--") => {}
            a if a.starts_with('-') && a.len() > 1 => {
                if let Some(value) = short_flag_value(a, create, &mut rest) {
                    new = Some(value?);
                }
            }
            a => positionals.push(a.to_owned()),
        }
    }
    let new = ref_name(new?)?;
    let parent = start_point(positionals.first(), &Creator::Checkout)?;
    Some(BranchCreation {
        repo: repo.to_path_buf(),
        new,
        parent,
    })
}

const BRANCH_CREATE_FLAGS: [&str; 10] = [
    "-f",
    "--force",
    "-t",
    "--track",
    "--no-track",
    "-q",
    "--quiet",
    "--create-reflog",
    "--recurse-submodules",
    "--no-recurse-submodules",
];

fn branching(args: &[String], repo: &Path) -> Option<BranchCreation> {
    let mut positionals = Vec::new();
    for arg in args {
        match arg.as_str() {
            a if BRANCH_CREATE_FLAGS.contains(&a) || a.starts_with("--track=") => {}
            a if a.starts_with('-') => return None,
            a => positionals.push(a.to_owned()),
        }
    }
    let (new, start) = match positionals.as_slice() {
        [new] => (new, None),
        [new, start] => (new, Some(start)),
        _ => return None,
    };
    Some(BranchCreation {
        repo: repo.to_path_buf(),
        new: ref_name(new)?,
        parent: start_point(start, &Creator::Branch)?,
    })
}

const WORKTREE_VALUE_FLAGS: [&str; 1] = ["--reason"];

fn worktree(args: &[String], repo: &Path) -> Option<BranchCreation> {
    let (sub, args) = args.split_first()?;
    if sub != "add" {
        return None;
    }
    let mut rest = args.iter();
    let mut new = None;
    let mut positionals = Vec::new();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--" => positionals.extend(rest.by_ref().cloned()),
            "-d" | "--detach" | "--orphan" => return None,
            "-b" | "-B" => new = Some(rest.next()?.as_str()),
            a if WORKTREE_VALUE_FLAGS.contains(&a) => {
                rest.next()?;
            }
            a if a.starts_with('-') => {}
            a => positionals.push(a.to_owned()),
        }
    }
    let (path, start) = match positionals.as_slice() {
        [path] => (path, None),
        [path, start] => (path, Some(start)),
        _ => return None,
    };
    let new = match (new, start) {
        (Some(new), _) => new,
        (None, Some(_)) => return None,
        (None, None) => Path::new(path).file_name()?.to_str()?,
    };
    Some(BranchCreation {
        repo: repo.to_path_buf(),
        new: ref_name(new)?,
        parent: start_point(start, &Creator::Worktree)?,
    })
}

fn git_creation(args: &[String], dir: &Path, home: Option<&Path>) -> Option<BranchCreation> {
    let mut repo = dir.to_path_buf();
    let mut rest = args.iter();
    let sub = loop {
        match rest.next()?.as_str() {
            "-C" => repo = joined(&repo, rest.next()?, home)?,
            "-c" | "--git-dir" | "--work-tree" | "--namespace" | "--exec-path" => {
                rest.next()?;
            }
            a if a.starts_with('-') => {}
            sub => break sub,
        }
    };
    let args: Vec<String> = rest.cloned().collect();
    match sub {
        "checkout" => switching(&args, &['b', 'B'], &[], &repo),
        "switch" => switching(&args, &['c', 'C'], &["--create", "--force-create"], &repo),
        "branch" => branching(&args, &repo),
        "worktree" => worktree(&args, &repo),
        _ => None,
    }
}

fn joined(dir: &Path, target: &str, home: Option<&Path>) -> Option<PathBuf> {
    let expanded = match (target.strip_prefix('~'), home) {
        (Some(""), Some(home)) => home.to_path_buf(),
        (Some(rest), Some(home)) if rest.starts_with('/') => home.join(&rest[1..]),
        (Some(_), _) => return None,
        (None, _) if target.contains(['$', '`']) => return None,
        (None, _) => PathBuf::from(target),
    };
    Some(dir.join(expanded))
}

fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty()
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && !name.starts_with(|c: char| c.is_ascii_digit())
    })
}

pub fn branch_creations(script: &str, cwd: &Path, home: Option<&Path>) -> Vec<BranchCreation> {
    let mut dir = Some(cwd.to_path_buf());
    let mut found = Vec::new();
    for words in simple_commands(script) {
        let words: Vec<String> = words.into_iter().skip_while(|w| is_assignment(w)).collect();
        let Some((program, args)) = words.split_first() else {
            continue;
        };
        match program.as_str() {
            "cd" | "pushd" => {
                dir = match (args.first().map(String::as_str), &dir, home) {
                    (None, _, Some(home)) => Some(home.to_path_buf()),
                    (Some(target), Some(current), _) if target != "-" => {
                        joined(current, target, home)
                    }
                    _ => None,
                }
            }
            "git" => found.extend(dir.as_deref().and_then(|d| git_creation(args, d, home))),
            _ => {}
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn created(script: &str) -> Vec<(String, String, ParentRef)> {
        branch_creations(script, Path::new("/work/repo"), Some(Path::new("/home/me")))
            .into_iter()
            .map(|c| (c.repo.display().to_string(), c.new.to_string(), c.parent))
            .collect()
    }

    fn one(script: &str) -> Option<(String, ParentRef)> {
        match created(script).as_slice() {
            [(repo, new, parent)] if repo == "/work/repo" => Some((new.clone(), parent.clone())),
            [] => None,
            other => panic!("{script}: {other:?}"),
        }
    }

    fn start(s: &str) -> ParentRef {
        ParentRef::StartPoint(s.into())
    }

    use ParentRef::{CurrentHead, PreviousCheckout};

    #[test]
    fn creation_commands() {
        let cases: &[(&str, &str, ParentRef)] = &[
            ("git checkout -b feat", "feat", PreviousCheckout),
            ("git checkout -B feat", "feat", PreviousCheckout),
            ("git checkout -b feat main", "feat", start("main")),
            ("git checkout -b feat HEAD", "feat", PreviousCheckout),
            (
                "git checkout -q -b feat origin/main",
                "feat",
                start("origin/main"),
            ),
            ("git checkout -bfeat", "feat", PreviousCheckout),
            ("git checkout -qb feat", "feat", PreviousCheckout),
            (
                "git checkout --track -b feat origin/feat",
                "feat",
                start("origin/feat"),
            ),
            ("git switch -c feat", "feat", PreviousCheckout),
            ("git switch -C feat dev", "feat", start("dev")),
            ("git switch --create feat", "feat", PreviousCheckout),
            ("git switch --create=feat dev", "feat", start("dev")),
            ("git switch --force-create feat", "feat", PreviousCheckout),
            ("git branch feat", "feat", CurrentHead),
            ("git branch feat dev", "feat", start("dev")),
            ("git branch -f feat dev", "feat", start("dev")),
            (
                "git branch --track=inherit feat origin/x",
                "feat",
                start("origin/x"),
            ),
            ("git worktree add -b feat ../wt", "feat", CurrentHead),
            ("git worktree add -B feat ../wt dev", "feat", start("dev")),
            (
                "git worktree add --lock --reason why -b feat ../wt",
                "feat",
                CurrentHead,
            ),
            ("git worktree add ../me/sc-12-wt", "sc-12-wt", CurrentHead),
            ("git worktree add -f ../wt", "wt", CurrentHead),
            (
                "git -c core.x=1 --no-pager checkout -b feat",
                "feat",
                PreviousCheckout,
            ),
            ("FOO=1 git switch -c 'me/sc-1/x y'", "", PreviousCheckout),
            ("git switch -c \"me/sc-1/x\"", "me/sc-1/x", PreviousCheckout),
            (
                "git checkout -b feat 2>&1 | tail -1",
                "feat",
                PreviousCheckout,
            ),
        ];
        for (script, new, parent) in cases {
            let expected = (!new.is_empty()).then(|| (new.to_string(), parent.clone()));
            assert_eq!(one(script), expected, "{script}");
        }
    }

    #[test]
    fn non_creation_commands() {
        for script in [
            "git branch",
            "git branch -a",
            "git branch --list 'sc-*'",
            "git branch -d feat",
            "git branch -D feat",
            "git branch -m old new",
            "git branch -M new",
            "git branch -c a b",
            "git branch --show-current",
            "git branch -u origin/x",
            "git branch --set-upstream-to=origin/x feat",
            "git branch --contains abc",
            "git branch -vv",
            "git branch a b c",
            "git checkout feat",
            "git checkout -- file.rs",
            "git checkout --orphan gh-pages",
            "git checkout -b",
            "git checkout -b -q",
            "git switch feat",
            "git switch -d HEAD~1",
            "git switch --orphan x",
            "git worktree add ../wt existing",
            "git worktree add --detach ../wt",
            "git worktree add -d ../wt",
            "git worktree list",
            "git worktree remove ../wt",
            "git log --oneline -b x",
            "echo git checkout -b x",
            "gitk branch x",
            "git branch $(whoami)/x",
            "git branch 'bad name'",
            "git branch a..b",
            "git branch x~1",
            "git checkout -b feat --orphan",
            "cd - && git branch x",
        ] {
            assert_eq!(one(script), None, "{script}");
        }
    }

    #[test]
    fn directories_follow_cd_and_dash_c() {
        assert_eq!(
            created(
                "cd ../other && git checkout -b a; git -C sub branch b; cd ~/r && git -C /abs switch -c c"
            ),
            vec![
                ("/work/repo/../other".into(), "a".into(), PreviousCheckout),
                ("/work/repo/../other/sub".into(), "b".into(), CurrentHead),
                ("/abs".into(), "c".into(), PreviousCheckout),
            ]
        );
        assert_eq!(
            created("cd && git branch x"),
            vec![("/home/me".into(), "x".into(), CurrentHead)]
        );
        assert_eq!(created("cd $DIR && git branch x"), vec![]);
        assert_eq!(
            created("git checkout -b one && git checkout -b two"),
            vec![
                ("/work/repo".into(), "one".into(), PreviousCheckout),
                ("/work/repo".into(), "two".into(), PreviousCheckout),
            ]
        );
    }
}
