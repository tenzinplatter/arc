use std::fmt;
use std::ops::Range;

use anyhow::{Context, bail};
use arc_core::{EpicId, IterationId, StoryId};
use chrono::NaiveDate;
use serde::{Deserialize, Deserializer, Serialize};
use serde_yaml::{Mapping, Value};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntityLink {
    None,
    Story {
        id: StoryId,
        url: String,
        name: Option<String>,
        iteration_url: Option<String>,
        epic_url: Option<String>,
    },
    Iteration {
        id: IterationId,
        url: String,
        name: Option<String>,
    },
    Epic {
        id: EpicId,
        url: String,
        name: Option<String>,
    },
}

impl EntityLink {
    pub fn kind(&self) -> Option<&'static str> {
        match self {
            EntityLink::None => None,
            EntityLink::Story { .. } => Some("story"),
            EntityLink::Iteration { .. } => Some("iteration"),
            EntityLink::Epic { .. } => Some("epic"),
        }
    }

    pub fn id_raw(&self) -> Option<i32> {
        match self {
            EntityLink::None => None,
            EntityLink::Story { id, .. } => Some(id.get()),
            EntityLink::Iteration { id, .. } => Some(id.get()),
            EntityLink::Epic { id, .. } => Some(id.get()),
        }
    }

    pub fn url(&self) -> Option<&str> {
        match self {
            EntityLink::None => None,
            EntityLink::Story { url, .. }
            | EntityLink::Iteration { url, .. }
            | EntityLink::Epic { url, .. } => Some(url),
        }
    }

    pub fn name(&self) -> Option<&str> {
        match self {
            EntityLink::None => None,
            EntityLink::Story { name, .. }
            | EntityLink::Iteration { name, .. }
            | EntityLink::Epic { name, .. } => name.as_deref(),
        }
    }

    pub fn iteration_url(&self) -> Option<&str> {
        match self {
            EntityLink::Story { iteration_url, .. } => iteration_url.as_deref(),
            _ => None,
        }
    }

    pub fn epic_url(&self) -> Option<&str> {
        match self {
            EntityLink::Story { epic_url, .. } => epic_url.as_deref(),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Frontmatter {
    pub id: String,
    pub created: Option<NaiveDate>,
    pub note_type: Option<String>,
    pub tags: Vec<String>,
    pub aliases: Vec<String>,
    pub link: EntityLink,
    pub branches: Vec<String>,
}

impl Frontmatter {
    fn created_today(slug: String, link: EntityLink) -> Self {
        Self {
            id: slug,
            created: Some(arc_core::time::today()),
            note_type: None,
            tags: Vec::new(),
            aliases: Vec::new(),
            link,
            branches: Vec::new(),
        }
    }

    pub fn story(
        slug: String,
        story_id: StoryId,
        story_name: String,
        story_url: String,
        iteration_url: Option<String>,
    ) -> Self {
        Self::created_today(
            slug,
            EntityLink::Story {
                id: story_id,
                url: story_url,
                name: Some(story_name),
                iteration_url,
                epic_url: None,
            },
        )
    }

    pub fn iteration(slug: String, id: IterationId, name: String, url: String) -> Self {
        Self::created_today(slug, EntityLink::Iteration { id, url, name: Some(name) })
    }

    pub fn epic(slug: String, id: EpicId, name: String, url: String) -> Self {
        Self::created_today(slug, EntityLink::Epic { id, url, name: Some(name) })
    }

    pub fn general(slug: String, tags: Vec<String>) -> Self {
        Self {
            tags,
            ..Self::created_today(slug, EntityLink::None)
        }
    }

    pub fn daily(slug: String) -> Self {
        Self {
            note_type: Some("daily".into()),
            ..Self::created_today(slug, EntityLink::None)
        }
    }

    pub fn scratch(slug: String) -> Self {
        Self {
            note_type: Some("scratch".into()),
            ..Self::created_today(slug, EntityLink::None)
        }
    }

    pub fn from_note(content: &str) -> Option<Frontmatter> {
        SplitNote::new(content)?.frontmatter()
    }

    pub fn to_block(&self) -> anyhow::Result<String> {
        let raw = RawFrontmatter::from(self.clone());
        Ok(format!("---\n{}---\n", serde_yaml::to_string(&raw)?))
    }
}

#[derive(Clone, Debug)]
pub struct SplitNote<'a> {
    content: &'a str,
    yaml: Range<usize>,
    body_start: usize,
    newline: &'static str,
}

impl<'a> SplitNote<'a> {
    /// Fences are whole lines after an optional BOM; an indented `---` or a `----` is YAML content.
    pub fn new(content: &'a str) -> Option<Self> {
        let bom = if content.starts_with('\u{feff}') {
            '\u{feff}'.len_utf8()
        } else {
            0
        };
        let mut lines = content[bom..].split_inclusive('\n');
        let open = lines.next()?;
        if !open.ends_with('\n') || open.trim_end() != "---" {
            return None;
        }
        let newline = if open.ends_with("\r\n") { "\r\n" } else { "\n" };
        let yaml_start = bom + open.len();
        let mut pos = yaml_start;
        for line in lines {
            if line.trim_end() == "---" {
                return Some(Self {
                    content,
                    yaml: yaml_start..pos,
                    body_start: pos + line.len(),
                    newline,
                });
            }
            pos += line.len();
        }
        None
    }

    pub fn yaml(&self) -> &'a str {
        &self.content[self.yaml.clone()]
    }

    pub fn body(&self) -> &'a str {
        &self.content[self.body_start..]
    }

    pub fn frontmatter(&self) -> Option<Frontmatter> {
        let raw: RawFrontmatter = serde_yaml::from_str(self.yaml()).ok()?;
        Some(raw.into())
    }

    pub fn with_yaml(&self, yaml: &str) -> String {
        format!(
            "{}{yaml}{}",
            &self.content[..self.yaml.start],
            &self.content[self.yaml.end..]
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BranchName(String);

impl BranchName {
    pub fn parse(name: &str) -> anyhow::Result<Self> {
        if name.is_empty() {
            bail!("empty branch name");
        }
        Ok(Self(name.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for BranchName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn branches_key() -> Value {
    Value::from("branches")
}

pub fn add_branch(content: &str, branch: &BranchName) -> anyhow::Result<String> {
    let note = SplitNote::new(content).context("note has no frontmatter block")?;
    let yaml = note.yaml();
    let before = parse_mapping(yaml)?;
    let existing = before.get(branches_key());
    let mut branches = existing.map(lenient_strings).unwrap_or_default();
    if branches.iter().any(|b| b == branch.as_str()) {
        return Ok(content.to_string());
    }

    let nl = note.newline;
    let item = yaml_scalar(branch.as_str());
    let new_yaml = match existing {
        None => format!("{yaml}branches:{nl}  - {item}{nl}"),
        Some(value) => edit_branches(yaml, value, &item, nl)?,
    };

    branches.push(branch.as_str().to_string());
    ensure_only_branches_changed(before, &new_yaml, &branches)?;
    Ok(note.with_yaml(&new_yaml))
}

fn ensure_only_branches_changed(
    mut before: Mapping,
    new_yaml: &str,
    want_branches: &[String],
) -> anyhow::Result<()> {
    let mut after = parse_mapping(new_yaml).context("edited frontmatter no longer parses")?;
    let got = after.get(branches_key()).map(lenient_strings).unwrap_or_default();
    before.remove(branches_key());
    after.remove(branches_key());
    if got != want_branches || before != after {
        bail!("refusing to edit frontmatter: adding the branch would disturb other values");
    }
    Ok(())
}

fn edit_branches(yaml: &str, value: &Value, item: &str, nl: &str) -> anyhow::Result<String> {
    let lines: Vec<(usize, &str)> = yaml
        .split_inclusive('\n')
        .scan(0, |off, l| {
            let start = *off;
            *off += l.len();
            Some((start, l))
        })
        .collect();
    let mut keys = lines
        .iter()
        .enumerate()
        .filter(|(_, (_, l))| l.starts_with("branches:"));
    let (Some((idx, &(start, line))), None) = (keys.next(), keys.next()) else {
        bail!("can't locate a single top-level `branches:` line");
    };
    let end = start + line.len();
    let rest = line["branches:".len()..].trim();
    let insert_after = |at: usize, text: String| format!("{}{text}{}", &yaml[..at], &yaml[at..]);
    let replace_line = |text: String| format!("{}{text}{}", &yaml[..start], &yaml[end..]);

    match value {
        Value::Null if rest.is_empty() || rest.starts_with('#') => {
            Ok(insert_after(end, format!("  - {item}{nl}")))
        }
        Value::Null if matches!(rest, "~" | "null" | "Null" | "NULL") => {
            Ok(replace_line(format!("branches:{nl}  - {item}{nl}")))
        }
        Value::String(_) => {
            if rest.is_empty()
                || rest.starts_with(['|', '>', '#', '&', '!', '*', '[', '{'])
                || rest.contains(" #")
            {
                bail!("`branches` scalar layout not supported");
            }
            Ok(replace_line(format!(
                "branches:{nl}  - {rest}{nl}  - {item}{nl}"
            )))
        }
        Value::Sequence(seq) if rest.starts_with('[') => {
            if !rest.ends_with(']') {
                bail!("`branches` flow list must be on one line");
            }
            if seq.is_empty() {
                return Ok(replace_line(format!("branches:{nl}  - {item}{nl}")));
            }
            let close = start + line.rfind(']').expect("ends with ]");
            Ok(insert_after(close, format!(", {item}")))
        }
        Value::Sequence(_) if rest.is_empty() || rest.starts_with('#') => {
            let block_list = lines[idx + 1..]
                .iter()
                .take_while(|(_, l)| l.trim().is_empty() || l.starts_with([' ', '\t', '-']));
            let mut indent = None;
            let mut last_end = None;
            for &(off, l) in block_list {
                let body = l.trim_start();
                if body.is_empty() || body.starts_with('#') {
                    continue;
                }
                if indent.is_none() && (body.starts_with("- ") || body.trim_end() == "-") {
                    indent = Some(&l[..l.len() - body.len()]);
                }
                last_end = Some(off + l.len());
            }
            let (Some(indent), Some(at)) = (indent, last_end) else {
                bail!("can't find `branches` list items");
            };
            Ok(insert_after(at, format!("{indent}- {item}{nl}")))
        }
        _ => bail!("`branches` layout not supported"),
    }
}

fn parse_mapping(yaml: &str) -> anyhow::Result<Mapping> {
    match serde_yaml::from_str(yaml)? {
        Value::Null => Ok(Default::default()),
        Value::Mapping(m) => Ok(m),
        _ => bail!("frontmatter is not a mapping"),
    }
}

fn yaml_scalar(s: &str) -> String {
    let plain = s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '/' | '.'))
        && serde_yaml::from_str::<Value>(s).ok() == Some(Value::String(s.into()));
    if plain {
        return s.to_string();
    }
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn lenient_strings(v: &Value) -> Vec<String> {
    let one = |v: &Value| match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    };
    match v {
        Value::Sequence(items) => items.iter().filter_map(one).collect(),
        v => one(v).into_iter().collect(),
    }
}

fn de_string_list<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    Ok(lenient_strings(&Value::deserialize(d)?))
}

fn parse_prefixed_id(s: &str) -> Option<i32> {
    s.rsplit('-').next()?.parse().ok()
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct RawFrontmatter {
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    #[serde(
        default,
        deserialize_with = "de_string_list",
        skip_serializing_if = "Vec::is_empty"
    )]
    aliases: Vec<String>,
    #[serde(
        default,
        deserialize_with = "de_string_list",
        skip_serializing_if = "Vec::is_empty"
    )]
    tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    created: Option<NaiveDate>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    note_type: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    story_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    story_link: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    story_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    iteration: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    epic: Option<String>,
    #[serde(
        default,
        deserialize_with = "de_string_list",
        skip_serializing_if = "Vec::is_empty"
    )]
    branches: Vec<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    iteration_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    iteration_link: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    iteration_name: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    epic_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    epic_link: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    epic_name: Option<String>,
}

impl From<RawFrontmatter> for Frontmatter {
    fn from(r: RawFrontmatter) -> Self {
        let link = if let Some(id) = r.story_id.as_deref().and_then(parse_prefixed_id) {
            EntityLink::Story {
                id: id.into(),
                url: r.story_link.unwrap_or_default(),
                name: r.story_name,
                iteration_url: r.iteration,
                epic_url: r.epic,
            }
        } else if let Some(id) = r.iteration_id.as_deref().and_then(parse_prefixed_id) {
            EntityLink::Iteration {
                id: id.into(),
                url: r.iteration_link.unwrap_or_default(),
                name: r.iteration_name,
            }
        } else if let Some(id) = r.epic_id.as_deref().and_then(parse_prefixed_id) {
            EntityLink::Epic {
                id: id.into(),
                url: r.epic_link.unwrap_or_default(),
                name: r.epic_name,
            }
        } else {
            EntityLink::None
        };

        Frontmatter {
            id: r.id.unwrap_or_default(),
            created: r.created,
            note_type: r.note_type,
            tags: r.tags,
            aliases: r.aliases,
            link,
            branches: r.branches,
        }
    }
}

impl From<Frontmatter> for RawFrontmatter {
    fn from(f: Frontmatter) -> Self {
        let mut raw = RawFrontmatter {
            id: Some(f.id),
            aliases: f.aliases,
            tags: f.tags,
            created: f.created,
            note_type: f.note_type,
            branches: f.branches,
            ..Default::default()
        };
        match f.link {
            EntityLink::None => {}
            EntityLink::Story {
                id,
                url,
                name,
                iteration_url,
                epic_url,
            } => {
                raw.story_id = Some(format!("sc-{id}"));
                raw.story_link = Some(url);
                raw.story_name = name;
                raw.iteration = iteration_url;
                raw.epic = epic_url;
            }
            EntityLink::Iteration { id, url, name } => {
                raw.iteration_id = Some(format!("it-{id}"));
                raw.iteration_link = Some(url);
                raw.iteration_name = name;
            }
            EntityLink::Epic { id, url, name } => {
                raw.epic_id = Some(format!("ep-{id}"));
                raw.epic_link = Some(url);
                raw.epic_name = name;
            }
        }
        raw
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn br(name: &str) -> BranchName {
        BranchName::parse(name).unwrap()
    }

    #[test]
    fn branch_name_rejects_only_empty() {
        assert!(BranchName::parse("").is_err());
        assert_eq!(BranchName::parse(" x ").unwrap().as_str(), " x ");
    }

    #[test]
    fn split_note_exposes_yaml_body_and_replacement() {
        let content = "\u{feff}---\r\nid: a\r\n---\r\nbody\r\n";
        let note = SplitNote::new(content).unwrap();
        assert_eq!(note.yaml(), "id: a\r\n");
        assert_eq!(note.body(), "body\r\n");
        assert_eq!(note.frontmatter().unwrap().id, "a");
        assert_eq!(note.with_yaml("id: b\r\n"), "\u{feff}---\r\nid: b\r\n---\r\nbody\r\n");
        assert!(SplitNote::new("---\n: [\n---\n").unwrap().frontmatter().is_none());
    }

    #[test]
    fn entity_link_columns_match_variant() {
        let story = EntityLink::Story {
            id: 7.into(),
            url: "https://s/7".into(),
            name: Some("S".into()),
            iteration_url: Some("https://it/3".into()),
            epic_url: None,
        };
        assert_eq!(story.kind(), Some("story"));
        assert_eq!(story.id_raw(), Some(7));
        assert_eq!(story.url(), Some("https://s/7"));
        assert_eq!(story.name(), Some("S"));
        assert_eq!(story.iteration_url(), Some("https://it/3"));
        assert_eq!(story.epic_url(), None);

        let iteration = EntityLink::Iteration { id: 3.into(), url: "u".into(), name: None };
        assert_eq!(iteration.kind(), Some("iteration"));
        assert_eq!(iteration.id_raw(), Some(3));
        assert_eq!(iteration.name(), None);
        assert_eq!(iteration.iteration_url(), None);

        let epic = EntityLink::Epic { id: 4.into(), url: "e".into(), name: Some("E".into()) };
        assert_eq!(epic.kind(), Some("epic"));
        assert_eq!(epic.url(), Some("e"));

        let none = EntityLink::None;
        assert_eq!(
            (none.kind(), none.id_raw(), none.url(), none.name(), none.iteration_url(), none.epic_url()),
            (None, None, None, None, None, None)
        );
    }

    #[test]
    fn reads_story_note() {
        let content = "---\nid: my-story\naliases: []\ntags: [a]\ncreated: \"2026-03-31\"\niteration: https://it/1\nstory_id: sc-19559\nstory_link: https://s/19559\n---\n\nbody";
        let fm = Frontmatter::from_note(content).unwrap();
        assert_eq!(fm.id, "my-story");
        assert_eq!(fm.tags, vec!["a"]);
        assert!(matches!(fm.link, EntityLink::Story { id, .. } if id.get() == 19559));
    }

    #[test]
    fn reads_bare_created_and_iteration_note() {
        let content = "---\niteration_id: it-17842\niteration_link: https://it/17842\ncreated: 2026-02-18\n---\n";
        let fm = Frontmatter::from_note(content).unwrap();
        assert_eq!(fm.created, NaiveDate::from_ymd_opt(2026, 2, 18));
        assert!(matches!(fm.link, EntityLink::Iteration { id, .. } if id.get() == 17842));
    }

    #[test]
    fn reads_plain_obsidian_note() {
        let content = "---\nid: rockpi\naliases: []\ntags: []\n---\n";
        let fm = Frontmatter::from_note(content).unwrap();
        assert_eq!(fm.id, "rockpi");
        assert_eq!(fm.link, EntityLink::None);
    }

    #[test]
    fn none_without_fence() {
        assert!(Frontmatter::from_note("# just a heading\n").is_none());
    }

    #[test]
    fn story_roundtrips_through_block() {
        let fm = Frontmatter::story(
            "s".into(),
            42.into(),
            "Name".into(),
            "https://s/42".into(),
            Some("https://it/1".into()),
        );
        let block = fm.to_block().unwrap();
        let full = format!("{block}\nbody");
        let back = Frontmatter::from_note(&full).unwrap();
        assert!(matches!(back.link, EntityLink::Story { id, .. } if id.get() == 42));
        assert!(block.contains("story_id: sc-42"));
    }

    #[test]
    fn branches_roundtrip_through_block() {
        let mut fm = Frontmatter::story("s".into(), 42.into(), "Name".into(), "https://s/42".into(), None);
        assert!(!fm.to_block().unwrap().contains("branches"));
        fm.branches = vec!["me/sc-42/name".into()];
        let full = format!("{}\nbody", fm.to_block().unwrap());
        let back = Frontmatter::from_note(&full).unwrap();
        assert_eq!(back.branches, vec!["me/sc-42/name"]);
    }

    #[test]
    fn add_branch_is_idempotent_and_keeps_body() {
        let content = "---\nid: s\nstory_id: sc-42\n---\n\n# Title\n\nbody\n";
        let once = add_branch(content, &br("feat-x")).unwrap();
        let twice = add_branch(&once, &br("feat-x")).unwrap();
        assert_eq!(once, twice);
        assert!(once.ends_with("\n---\n\n# Title\n\nbody\n"));
        let fm = Frontmatter::from_note(&once).unwrap();
        assert_eq!(fm.branches, vec!["feat-x"]);
        assert!(matches!(fm.link, EntityLink::Story { id, .. } if id.get() == 42));

        let more = add_branch(&once, &br("feat-y")).unwrap();
        assert_eq!(
            Frontmatter::from_note(&more).unwrap().branches,
            vec!["feat-x", "feat-y"]
        );
    }

    #[test]
    fn add_branch_preserves_unknown_keys() {
        let content = "---\nid: s\ncssclasses: [wide]\npublish: true\nbranches:\n---\nbody";
        let out = add_branch(content, &br("b")).unwrap();
        assert_eq!(
            out,
            "---\nid: s\ncssclasses: [wide]\npublish: true\nbranches:\n  - b\n---\nbody"
        );
        assert_eq!(Frontmatter::from_note(&out).unwrap().branches, vec!["b"]);
    }

    #[test]
    fn fences_are_whole_lines() {
        let content = "---\n---\n# Meeting\nNote: remember X\n\n---\nfooter\n";
        assert_eq!(SplitNote::new(content).map(|n| n.yaml()), Some(""));
        let out = add_branch(content, &br("b")).unwrap();
        assert_eq!(
            out,
            "---\nbranches:\n  - b\n---\n# Meeting\nNote: remember X\n\n---\nfooter\n"
        );

        let content = "---\nid: a\nnote: |\n  ---\n  x\n----\n---\nbody";
        assert_eq!(
            SplitNote::new(content).map(|n| n.yaml()),
            Some("id: a\nnote: |\n  ---\n  x\n----\n")
        );
        assert!(SplitNote::new("---\nid: a\n").map(|n| n.yaml()).is_none());
        assert!(SplitNote::new("--- \nid: a\n---  \n").map(|n| n.yaml()).is_some());
        assert!(SplitNote::new("x\n---\nid: a\n---\n").map(|n| n.yaml()).is_none());
    }

    #[test]
    fn reads_crlf_and_bom() {
        let fm =
            Frontmatter::from_note("\u{feff}---\r\nid: a\r\ntags: [x]\r\n---\r\nbody\r\n").unwrap();
        assert_eq!(fm.id, "a");
        assert_eq!(fm.tags, vec!["x"]);
    }

    #[test]
    fn lists_read_leniently() {
        let fm = Frontmatter::from_note("---\nid: a\nbranches: main\ntags: foo\naliases:\n---\n")
            .unwrap();
        assert_eq!(fm.branches, vec!["main"]);
        assert_eq!(fm.tags, vec!["foo"]);
        assert!(fm.aliases.is_empty());

        let fm = Frontmatter::from_note(
            "---\nid: a\nbranches: {x: y}\ntags: [a, 1, {k: v}, [n], true]\nstory_id: sc-3\n---\n",
        )
        .unwrap();
        assert!(fm.branches.is_empty());
        assert_eq!(fm.tags, vec!["a", "1"]);
        assert!(matches!(fm.link, EntityLink::Story { id, .. } if id.get() == 3));
    }

    #[test]
    fn add_branch_leaves_other_bytes_alone() {
        let head = "---\n# arc metadata\nid: s # slug\ncreated: \"2026-03-31\"\nversion: 1.10\nsummary: >\n  folded\n  text\n";
        let body = "---\n\n# Title\n\n---\nbody with trailing spaces  \n";
        let out = add_branch(&format!("{head}{body}"), &br("feat/x")).unwrap();
        assert_eq!(out, format!("{head}branches:\n  - feat/x\n{body}"));
    }

    #[test]
    fn add_branch_appends_to_block_list() {
        let content = "---\nid: s\nbranches:\n    - a # first\n    - b\n\ntags: [t]\n---\nbody";
        assert_eq!(
            add_branch(content, &br("c")).unwrap(),
            "---\nid: s\nbranches:\n    - a # first\n    - b\n    - c\n\ntags: [t]\n---\nbody"
        );
        let content = "---\nbranches:\n- a\n---\n";
        assert_eq!(
            add_branch(content, &br("c")).unwrap(),
            "---\nbranches:\n- a\n- c\n---\n"
        );
    }

    #[test]
    fn add_branch_flow_empty_and_scalar() {
        assert_eq!(
            add_branch("---\nbranches: [a, 'b']\nx: 1\n---\n", &br("c")).unwrap(),
            "---\nbranches: [a, 'b', c]\nx: 1\n---\n"
        );
        assert_eq!(
            add_branch("---\nbranches: []\nx: 1\n---\n", &br("c")).unwrap(),
            "---\nbranches:\n  - c\nx: 1\n---\n"
        );
        assert_eq!(
            add_branch("---\nbranches: ~\n---\n", &br("c")).unwrap(),
            "---\nbranches:\n  - c\n---\n"
        );
        assert_eq!(
            add_branch("---\nbranches: main\nx: 1\n---\n", &br("c")).unwrap(),
            "---\nbranches:\n  - main\n  - c\nx: 1\n---\n"
        );
        let content = "---\nbranches: main\n---\n";
        assert_eq!(add_branch(content, &br("main")).unwrap(), content);
    }

    #[test]
    fn add_branch_crlf_and_bom() {
        let content = "\u{feff}---\r\nid: s\r\nbranches:\r\n  - a\r\n---\r\nbody\r\n";
        let out = add_branch(content, &br("b")).unwrap();
        assert_eq!(
            out,
            "\u{feff}---\r\nid: s\r\nbranches:\r\n  - a\r\n  - b\r\n---\r\nbody\r\n"
        );
        assert_eq!(add_branch(&out, &br("b")).unwrap(), out);

        let out = add_branch("---\r\nid: s\r\n---\r\n", &br("b")).unwrap();
        assert_eq!(out, "---\r\nid: s\r\nbranches:\r\n  - b\r\n---\r\n");
    }

    #[test]
    fn add_branch_quotes_ambiguous_names() {
        let out = add_branch("---\nid: s\n---\n", &br("true")).unwrap();
        assert!(out.contains("  - \"true\"\n"));
        let out = add_branch(&out, &br("a: b#c")).unwrap();
        assert!(out.contains("  - \"a: b#c\"\n"));
        assert_eq!(
            Frontmatter::from_note(&out).unwrap().branches,
            vec!["true", "a: b#c"]
        );
        let out = add_branch("---\nbranches: [a]\n---\n", &br("x,y")).unwrap();
        assert_eq!(out, "---\nbranches: [a, \"x,y\"]\n---\n");
    }

    #[test]
    fn add_branch_refuses_unclear_layouts() {
        for content in [
            "---\nbranches: {a: b}\n---\n",
            "---\nbranches: |\n  a\n---\n",
            "---\nbranches: [a,\n  b]\n---\n",
            "---\nbranches: [a] # c\n---\n",
            "---\n\"branches\": [a]\n---\n",
            "---\n- just\n- a list\n---\n",
            "---\nid: [unclosed\n---\n",
            "---\nbranches:\n  - a\n# interrupting comment\n  - b\n---\n",
        ] {
            assert!(add_branch(content, &br("z")).is_err(), "{content:?}");
        }
    }

    #[test]
    fn add_branch_requires_frontmatter() {
        assert!(add_branch("# no fence\n", &br("b")).is_err());
        assert!(add_branch("---\nid: s\n", &br("b")).is_err());
    }

    fn pinned_block(mut fm: Frontmatter) -> String {
        assert_eq!(fm.created, Some(arc_core::time::today()));
        fm.created = NaiveDate::from_ymd_opt(2026, 3, 31);
        fm.to_block().unwrap()
    }

    #[test]
    fn golden_story_block_with_iteration() {
        let fm = Frontmatter::story(
            "sc-7-slug".into(),
            7.into(),
            "Story seven".into(),
            "https://s/7".into(),
            Some("https://it/3".into()),
        );
        assert_eq!(
            pinned_block(fm),
            "---\nid: sc-7-slug\ncreated: 2026-03-31\nstory_id: sc-7\nstory_link: https://s/7\nstory_name: Story seven\niteration: https://it/3\n---\n"
        );
    }

    #[test]
    fn golden_story_block_without_iteration() {
        let fm = Frontmatter::story("sc-8-slug".into(), 8.into(), "Story eight".into(), "https://s/8".into(), None);
        assert_eq!(
            pinned_block(fm),
            "---\nid: sc-8-slug\ncreated: 2026-03-31\nstory_id: sc-8\nstory_link: https://s/8\nstory_name: Story eight\n---\n"
        );
    }

    #[test]
    fn golden_iteration_block() {
        let fm = Frontmatter::iteration("it-3-slug".into(), 3.into(), "Sprint 3".into(), "https://it/3".into());
        assert_eq!(
            pinned_block(fm),
            "---\nid: it-3-slug\ncreated: 2026-03-31\niteration_id: it-3\niteration_link: https://it/3\niteration_name: Sprint 3\n---\n"
        );
    }

    #[test]
    fn golden_epic_block() {
        let fm = Frontmatter::epic("ep-40-slug".into(), 40.into(), "Epic forty".into(), "https://ep/40".into());
        assert_eq!(
            pinned_block(fm),
            "---\nid: ep-40-slug\ncreated: 2026-03-31\nepic_id: ep-40\nepic_link: https://ep/40\nepic_name: Epic forty\n---\n"
        );
    }

    #[test]
    fn golden_general_block() {
        let fm = Frontmatter::general("general-slug".into(), vec!["a".into(), "b".into()]);
        assert_eq!(
            pinned_block(fm),
            "---\nid: general-slug\ntags:\n- a\n- b\ncreated: 2026-03-31\n---\n"
        );
    }

    #[test]
    fn golden_daily_block() {
        assert_eq!(
            pinned_block(Frontmatter::daily("2026-03-31".into())),
            "---\nid: 2026-03-31\ncreated: 2026-03-31\ntype: daily\n---\n"
        );
    }

    #[test]
    fn golden_scratch_block() {
        assert_eq!(
            pinned_block(Frontmatter::scratch("scratch-slug".into())),
            "---\nid: scratch-slug\ncreated: 2026-03-31\ntype: scratch\n---\n"
        );
    }

    #[test]
    fn golden_block_with_every_field_set() {
        let fm = Frontmatter {
            id: "sc-9-slug".into(),
            created: NaiveDate::from_ymd_opt(2026, 3, 31),
            note_type: Some("story".into()),
            tags: vec!["t".into()],
            aliases: vec!["alias".into()],
            link: EntityLink::Story {
                id: 9.into(),
                url: "https://s/9".into(),
                name: Some("Story nine".into()),
                iteration_url: Some("https://it/3".into()),
                epic_url: Some("https://ep/40".into()),
            },
            branches: vec!["sc-9-branch".into()],
        };
        assert_eq!(
            fm.to_block().unwrap(),
            "---\nid: sc-9-slug\naliases:\n- alias\ntags:\n- t\ncreated: 2026-03-31\ntype: story\nstory_id: sc-9\nstory_link: https://s/9\nstory_name: Story nine\niteration: https://it/3\nepic: https://ep/40\nbranches:\n- sc-9-branch\n---\n"
        );
    }
}
