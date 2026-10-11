use std::fmt;

use anyhow::{anyhow, bail};
use serde::{Deserialize, Serialize};

use crate::checkbox::split_checkbox;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Description(String);

pub(crate) fn leading_width(line: &str) -> usize {
    line.len() - line.trim_start_matches([' ', '\t']).len()
}

impl Description {
    pub fn parse(raw: &str) -> anyhow::Result<Option<Self>> {
        let lines: Vec<&str> = raw.lines().filter(|line| !line.trim().is_empty()).collect();
        if let Some(line) = lines.iter().find(|line| split_checkbox(line.trim_start()).is_some()) {
            bail!("a description line can't be a checkbox: `{}`", line.trim());
        }
        let Some(common) = lines.iter().map(|line| leading_width(line)).min() else {
            return Ok(None);
        };
        let dedented: Vec<&str> = lines.iter().map(|line| line[common..].trim_end()).collect();
        Ok(Some(Self(dedented.join("\n"))))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn indented(&self, indent: &str) -> impl Iterator<Item = String> {
        self.0.lines().map(move |line| format!("{indent}{line}"))
    }
}

impl TryFrom<String> for Description {
    type Error = anyhow::Error;

    fn try_from(raw: String) -> anyhow::Result<Self> {
        Self::parse(&raw)?.ok_or_else(|| anyhow!("a description can't be blank"))
    }
}

impl From<Description> for String {
    fn from(description: Description) -> Self {
        description.0
    }
}

impl fmt::Display for Description {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_blank_lines_and_the_common_indent() {
        let parsed = Description::parse("\n    first\n\n      nested\n    last  \n").unwrap().unwrap();
        assert_eq!(parsed.as_str(), "first\n  nested\nlast");
    }

    #[test]
    fn blank_input_is_no_description() {
        assert_eq!(Description::parse(" \n\t\n").unwrap(), None);
        assert!(Description::try_from(String::new()).is_err());
    }

    #[test]
    fn rejects_checkbox_lines_which_would_parse_back_as_todos() {
        assert!(Description::parse("context\n- [ ] sneaky").is_err());
    }
}
