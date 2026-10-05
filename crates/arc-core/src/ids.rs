use std::fmt;
use std::str::FromStr;

use anyhow::{Context, bail};

use serde::{Deserialize, Serialize};

macro_rules! int_id {
    ($name:ident) => {
        #[derive(
            Copy,
            Clone,
            Debug,
            Default,
            PartialEq,
            Eq,
            PartialOrd,
            Ord,
            Hash,
            Serialize,
            Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(i32);

        impl $name {
            pub fn get(self) -> i32 {
                self.0
            }
        }

        impl From<i32> for $name {
            fn from(id: i32) -> Self {
                Self(id)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

int_id!(StoryId);
int_id!(IterationId);
int_id!(EpicId);

const STORY_PREFIX: &str = "sc-";

impl FromStr for StoryId {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> anyhow::Result<Self> {
        let digits = match s.get(..STORY_PREFIX.len()) {
            Some(prefix) if prefix.eq_ignore_ascii_case(STORY_PREFIX) => &s[STORY_PREFIX.len()..],
            _ => s,
        };
        let id: i32 = digits
            .bytes()
            .all(|b| b.is_ascii_digit())
            .then(|| digits.parse().ok())
            .flatten()
            .with_context(|| format!("'{s}' is not a story id like 123 or sc-123"))?;
        if id <= 0 {
            bail!("story id must be positive, got {id}");
        }
        Ok(Self(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn story_ids_parse_bare_or_prefixed() {
        let parse = |s: &str| s.parse::<StoryId>().ok().map(StoryId::get);
        assert_eq!(parse("123"), Some(123));
        assert_eq!(parse("sc-123"), Some(123));
        assert_eq!(parse("SC-7"), Some(7));
        assert_eq!(parse("Sc-007"), Some(7));
        assert_eq!(parse("0"), None);
        assert_eq!(parse("sc-0"), None);
        assert_eq!(parse("-5"), None);
        assert_eq!(parse("sc--5"), None);
        assert_eq!(parse("sc-"), None);
        assert_eq!(parse("sc12"), None);
        assert_eq!(parse("12x"), None);
        assert_eq!(parse("+5"), None);
        assert_eq!(parse("99999999999"), None);
        assert_eq!(parse(""), None);
    }
}
