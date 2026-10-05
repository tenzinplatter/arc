use std::fmt;

use anyhow::bail;
use arc_core::StoryId;
use arc_notes::BranchName;
use arc_shortcut::api::user::MentionName;
use slugify::slugify;

const STORY_PREFIX: &str = "sc-";
const TRUNK_BRANCHES: [&str; 4] = ["main", "master", "develop", "trunk"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkableBranch(BranchName);

impl LinkableBranch {
    pub fn parse(branch: BranchName, default_branch: Option<&BranchName>) -> anyhow::Result<Self> {
        if TRUNK_BRANCHES.contains(&branch.as_str()) || default_branch == Some(&branch) {
            bail!(
                "'{branch}' is a trunk branch; links apply to every repo with a branch of that name, so link a story branch instead"
            );
        }
        Ok(Self(branch))
    }

    pub fn as_branch(&self) -> &BranchName {
        &self.0
    }

    pub fn into_branch(self) -> BranchName {
        self.0
    }
}

impl fmt::Display for LinkableBranch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

pub fn story_id_in_branch(branch: &BranchName) -> Option<StoryId> {
    let lower = branch.as_str().to_ascii_lowercase();
    lower
        .match_indices(STORY_PREFIX)
        .filter(|&(at, _)| !lower[..at].ends_with(|c: char| c.is_ascii_alphanumeric()))
        .find_map(|(at, _)| {
            let rest = &lower[at + STORY_PREFIX.len()..];
            let digits = rest
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(rest.len());
            let bounded = !rest[digits..].starts_with(|c: char| c.is_ascii_alphanumeric());
            let id: i32 = rest[..digits]
                .parse()
                .ok()
                .filter(|&id| bounded && id > 0)?;
            Some(StoryId::from(id))
        })
}

pub fn canonical_branch(
    mention_name: &MentionName,
    id: StoryId,
    story_name: &str,
    suffix: Option<&str>,
) -> anyhow::Result<BranchName> {
    let tail = [Some(slugify!(story_name)), suffix.map(|s| slugify!(s))]
        .into_iter()
        .flatten()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let base = format!("{mention_name}/{STORY_PREFIX}{id}");
    BranchName::parse(&match tail.as_str() {
        "" => base,
        tail => format!("{base}/{tail}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id_in(branch: &str) -> Option<i32> {
        story_id_in_branch(&BranchName::parse(branch).unwrap()).map(StoryId::get)
    }

    #[test]
    fn finds_story_ids_in_branch_names() {
        assert_eq!(
            id_in("dangr110/sc-16727/optical-flow-track-compensation"),
            Some(16727)
        );
        assert_eq!(id_in("feature/sc-12/x"), Some(12));
        assert_eq!(id_in("sc-12"), Some(12));
        assert_eq!(id_in("sc-12-fix-thing"), Some(12));
        assert_eq!(id_in("fix_sc-12"), Some(12));
        assert_eq!(id_in("SC-12/upper"), Some(12));
        assert_eq!(id_in("sc-007"), Some(7));
    }

    #[test]
    fn rejects_non_story_tokens() {
        assert_eq!(id_in("sc-0"), None);
        assert_eq!(id_in("sc-123abc"), None);
        assert_eq!(id_in("disc-12"), None);
        assert_eq!(id_in("sc-"), None);
        assert_eq!(id_in("sc-x"), None);
        assert_eq!(id_in("sc-99999999999"), None);
        assert_eq!(id_in("main"), None);
    }

    #[test]
    fn first_valid_match_wins() {
        assert_eq!(id_in("me/sc-12/stacked-on-sc-13"), Some(12));
        assert_eq!(id_in("sc-0/sc-5"), Some(5));
        assert_eq!(id_in("sc-1abc/sc-9"), Some(9));
    }

    fn mention(name: &str) -> MentionName {
        MentionName::parse(name).unwrap()
    }

    fn canonical(name_of: &str, id: i32, name: &str, suffix: Option<&str>) -> String {
        canonical_branch(&mention(name_of), id.into(), name, suffix)
            .unwrap()
            .to_string()
    }

    #[test]
    fn canonical_branch_names() {
        assert_eq!(
            canonical("dangr110", 16727, "Optical flow: track compensation", None),
            "dangr110/sc-16727/optical-flow-track-compensation"
        );
        assert_eq!(
            canonical("tenzin", 7, "Story Seven", Some("Part 2")),
            "tenzin/sc-7/story-seven-part-2"
        );
        assert_eq!(canonical("tenzin", 7, "!!!", None), "tenzin/sc-7");
        assert_eq!(canonical("tenzin", 7, "!!!", Some("v2")), "tenzin/sc-7/v2");
        assert_eq!(
            canonical("tenzin", 7, "Seven", Some("")),
            "tenzin/sc-7/seven"
        );
    }

    #[test]
    fn canonical_branch_resolves_back_to_its_story() {
        let branch =
            canonical_branch(&mention("tenzin"), 16727.into(), "sc-1 regression", None).unwrap();
        assert_eq!(story_id_in_branch(&branch), Some(16727.into()));
    }

    fn linkable(name: &str, default: Option<&str>) -> anyhow::Result<LinkableBranch> {
        let default = default.map(|d| BranchName::parse(d).unwrap());
        LinkableBranch::parse(BranchName::parse(name).unwrap(), default.as_ref())
    }

    #[test]
    fn trunk_branches_are_not_linkable() {
        for trunk in TRUNK_BRANCHES {
            let err = linkable(trunk, None).unwrap_err();
            assert!(err.to_string().contains("trunk branch"), "{err}");
        }
        assert!(linkable("release", Some("release")).is_err());
        assert_eq!(
            linkable("feature/x", Some("release"))
                .unwrap()
                .into_branch(),
            BranchName::parse("feature/x").unwrap()
        );
        assert!(linkable("main-fix", None).is_ok());
        assert!(linkable("me/main", Some("main")).is_ok());
    }
}
