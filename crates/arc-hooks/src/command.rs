use std::path::{Path, PathBuf};

use arc_notes::BranchName;

use crate::shell::simple_commands;

const REF_FORBIDDEN: [char; 11] = [' ', '~', '^', ':', '?', '*', '[', '\\', '$', '`', '\''];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchCreation {
    pub repo: PathBuf,
    pub new: BranchName,
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

fn plausible_start(word: Option<&String>) -> bool {
    word.is_none_or(|start| !start.starts_with('-'))
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
    plausible_start(positionals.first()).then(|| BranchCreation {
        repo: repo.to_path_buf(),
        new,
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
    if !plausible_start(start) {
        return None;
    }
    Some(BranchCreation {
        repo: repo.to_path_buf(),
        new: ref_name(new)?,
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
    match positionals.as_slice() {
        [_] | [_, _] => Some(BranchCreation {
            repo: repo.to_path_buf(),
            new: ref_name(new?)?,
        }),
        _ => None,
    }
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

    fn created(script: &str) -> Vec<(String, String)> {
        branch_creations(script, Path::new("/work/repo"), Some(Path::new("/home/me")))
            .into_iter()
            .map(|c| (c.repo.display().to_string(), c.new.to_string()))
            .collect()
    }

    fn one(script: &str) -> Option<String> {
        match created(script).as_slice() {
            [(repo, new)] if repo == "/work/repo" => Some(new.clone()),
            [] => None,
            other => panic!("{script}: {other:?}"),
        }
    }

    #[test]
    fn creation_commands() {
        let cases: &[(&str, &str)] = &[
            ("git checkout -b feat", "feat"),
            ("git checkout -B feat", "feat"),
            ("git checkout -b feat main", "feat"),
            ("git checkout -b feat HEAD", "feat"),
            ("git checkout -q -b feat origin/main", "feat"),
            ("git checkout -bfeat", "feat"),
            ("git checkout -qb feat", "feat"),
            ("git checkout --track -b feat origin/feat", "feat"),
            ("git switch -c feat", "feat"),
            ("git switch -C feat dev", "feat"),
            ("git switch --create feat", "feat"),
            ("git switch --create=feat dev", "feat"),
            ("git switch --force-create feat", "feat"),
            ("git branch feat", "feat"),
            ("git branch feat dev", "feat"),
            ("git branch -f feat dev", "feat"),
            ("git branch --track=inherit feat origin/x", "feat"),
            ("git worktree add -b feat ../wt", "feat"),
            ("git worktree add -B feat ../wt dev", "feat"),
            ("git worktree add --lock --reason why -b feat ../wt", "feat"),
            ("git -c core.x=1 --no-pager checkout -b feat", "feat"),
            ("FOO=1 git switch -c 'me/sc-1/x y'", ""),
            ("git switch -c \"me/sc-1/x\"", "me/sc-1/x"),
            ("git checkout -b feat 2>&1 | tail -1", "feat"),
            ("cat <<EOF\nnotes\nEOF\ngit branch feat", "feat"),
        ];
        for (script, new) in cases {
            let expected = (!new.is_empty()).then(|| new.to_string());
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
            "git worktree add ../me/sc-12-wt",
            "git worktree add -f ../wt",
            "cat <<EOF\ngit checkout -b x\nEOF",
            "cat <<-'END' > notes.md\n\tgit branch x\n\tEND\n",
            "git commit -F- <<\"MSG\"\ngit switch -c x\nMSG",
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
        let at = |repo: &str, new: &str| (repo.to_owned(), new.to_owned());
        assert_eq!(
            created(
                "cd ../other && git checkout -b a; git -C sub branch b; cd ~/r && git -C /abs switch -c c"
            ),
            vec![
                at("/work/repo/../other", "a"),
                at("/work/repo/../other/sub", "b"),
                at("/abs", "c"),
            ]
        );
        assert_eq!(created("cd && git branch x"), vec![at("/home/me", "x")]);
        assert_eq!(created("cd $DIR && git branch x"), vec![]);
        assert_eq!(
            created("git checkout -b one && git checkout -b two"),
            vec![at("/work/repo", "one"), at("/work/repo", "two")]
        );
    }
}
