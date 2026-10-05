#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Checkbox {
    Open,
    Done,
}

impl Checkbox {
    pub fn from_completed(completed: bool) -> Self {
        if completed { Checkbox::Done } else { Checkbox::Open }
    }

    pub fn is_done(self) -> bool {
        self == Checkbox::Done
    }

    pub fn marker(self) -> &'static str {
        match self {
            Checkbox::Open => "- [ ] ",
            Checkbox::Done => "- [x] ",
        }
    }
}

pub fn split_checkbox(line: &str) -> Option<(Checkbox, &str)> {
    if let Some(rest) = line.strip_prefix("- [ ] ") {
        return Some((Checkbox::Open, rest));
    }
    line.strip_prefix("- [x] ")
        .or_else(|| line.strip_prefix("- [X] "))
        .map(|rest| (Checkbox::Done, rest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_open_and_either_case_of_done() {
        assert_eq!(split_checkbox("- [ ] a"), Some((Checkbox::Open, "a")));
        assert_eq!(split_checkbox("- [x] b"), Some((Checkbox::Done, "b")));
        assert_eq!(split_checkbox("- [X] c"), Some((Checkbox::Done, "c")));
    }

    #[test]
    fn rejects_indented_or_unspaced_markers() {
        for line in ["  - [ ] a", "- [ ]a", "- [] a", "* [ ] a", "text"] {
            assert_eq!(split_checkbox(line), None, "{line}");
        }
    }
}
