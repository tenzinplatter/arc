use chrono::NaiveDate;
use serde::{Deserialize, Deserializer, Serialize};

/// The Shortcut entity a note is linked to. A note is about *at most one*
/// entity — the enum makes "linked to a story AND an epic id" unrepresentable.
/// (A story note still carries its iteration/epic *links* as context.)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntityLink {
    None,
    Story {
        id: i32,
        url: String,
        name: Option<String>,
        iteration_url: Option<String>,
        epic_url: Option<String>,
    },
    Iteration {
        id: i32,
        url: String,
        name: Option<String>,
    },
    Epic {
        id: i32,
        url: String,
        name: Option<String>,
    },
}

/// Parsed frontmatter of a note. Mirrors the shape already on disk (Obsidian
/// `id`/`aliases`/`tags` plus the Shortcut linkage), so reading is
/// non-destructive and writing stays Obsidian-compatible.
#[derive(Debug, Clone)]
pub struct Frontmatter {
    /// Obsidian slug — the filename stem.
    pub id: String,
    pub created: Option<NaiveDate>,
    pub note_type: Option<String>,
    pub tags: Vec<String>,
    pub aliases: Vec<String>,
    pub link: EntityLink,
    /// Git branches linked to this note (story notes).
    pub branches: Vec<String>,
}

impl Frontmatter {
    pub fn story(
        slug: String,
        story_id: i32,
        story_name: String,
        story_url: String,
        iteration_url: Option<String>,
    ) -> Self {
        Self {
            id: slug,
            created: Some(arc_core::time::today()),
            note_type: None,
            tags: Vec::new(),
            aliases: Vec::new(),
            branches: Vec::new(),
            link: EntityLink::Story {
                id: story_id,
                url: story_url,
                name: Some(story_name),
                iteration_url,
                epic_url: None,
            },
        }
    }

    pub fn iteration(slug: String, id: i32, name: String, url: String) -> Self {
        Self {
            id: slug,
            created: Some(arc_core::time::today()),
            note_type: None,
            tags: Vec::new(),
            aliases: Vec::new(),
            branches: Vec::new(),
            link: EntityLink::Iteration {
                id,
                url,
                name: Some(name),
            },
        }
    }

    pub fn epic(slug: String, id: i32, name: String, url: String) -> Self {
        Self {
            id: slug,
            created: Some(arc_core::time::today()),
            note_type: None,
            tags: Vec::new(),
            aliases: Vec::new(),
            branches: Vec::new(),
            link: EntityLink::Epic {
                id,
                url,
                name: Some(name),
            },
        }
    }

    /// A plain note with no Shortcut linkage (created via MCP, etc.).
    pub fn general(slug: String, tags: Vec<String>) -> Self {
        Self {
            id: slug,
            created: Some(arc_core::time::today()),
            note_type: None,
            tags,
            aliases: Vec::new(),
            branches: Vec::new(),
            link: EntityLink::None,
        }
    }

    pub fn daily(slug: String) -> Self {
        Self {
            id: slug,
            created: Some(arc_core::time::today()),
            note_type: Some("daily".into()),
            tags: Vec::new(),
            aliases: Vec::new(),
            branches: Vec::new(),
            link: EntityLink::None,
        }
    }

    pub fn scratch(slug: String) -> Self {
        Self {
            id: slug,
            created: Some(arc_core::time::today()),
            note_type: Some("scratch".into()),
            tags: Vec::new(),
            aliases: Vec::new(),
            branches: Vec::new(),
            link: EntityLink::None,
        }
    }

    /// Parse the frontmatter block out of a full note file. Returns `None` when
    /// the file has no leading `---` fence or the YAML doesn't parse.
    pub fn from_note(content: &str) -> Option<Frontmatter> {
        let yaml = extract_yaml(content)?;
        let raw: RawFrontmatter = serde_yaml::from_str(yaml).ok()?;
        Some(raw.into())
    }

    /// The `---\n…\n---\n` block for a fresh note.
    pub fn to_block(&self) -> anyhow::Result<String> {
        let raw = RawFrontmatter::from(self.clone());
        Ok(format!("---\n{}---\n", serde_yaml::to_string(&raw)?))
    }
}

/// Add `branch` to a note's `branches:` frontmatter. A text-level edit: every
/// other byte of the note (comments, quoting, other keys, body) is untouched.
/// No-op if already present. Errors rather than guess on layouts it can't
/// edit confidently.
pub fn add_branch(content: &str, branch: &str) -> anyhow::Result<String> {
    use anyhow::{Context, bail};

    if branch.is_empty() {
        bail!("empty branch name");
    }
    let fenced = split_frontmatter(content).context("note has no frontmatter block")?;
    let yaml = &content[fenced.yaml.clone()];
    let before = parse_mapping(yaml)?;
    let key = serde_yaml::Value::from("branches");
    let existing = before.get(&key);
    let old = existing.map(lenient_strings).unwrap_or_default();
    if old.iter().any(|b| b == branch) {
        return Ok(content.to_string());
    }

    let nl = fenced.newline;
    let item = yaml_scalar(branch);
    let new_yaml = match existing {
        None => format!("{yaml}branches:{nl}  - {item}{nl}"),
        Some(value) => edit_branches(yaml, value, &item, nl)?,
    };

    // Verify the edit did exactly one thing: appended `branch`.
    let after = parse_mapping(&new_yaml).context("edited frontmatter no longer parses")?;
    let got = after.get(&key).map(lenient_strings).unwrap_or_default();
    let mut want = old;
    want.push(branch.to_string());
    let without = |mut m: serde_yaml::Mapping| {
        m.remove(&key);
        m
    };
    if got != want || without(before) != without(after) {
        bail!("refusing to edit frontmatter: adding the branch would disturb other values");
    }

    Ok(format!(
        "{}{new_yaml}{}",
        &content[..fenced.yaml.start],
        &content[fenced.yaml.end..]
    ))
}

/// Rewrite an existing `branches:` entry in `yaml` to also list `item`.
fn edit_branches(
    yaml: &str,
    value: &serde_yaml::Value,
    item: &str,
    nl: &str,
) -> anyhow::Result<String> {
    use anyhow::bail;
    use serde_yaml::Value;

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
            // The block list: following blank/indented/`-` lines.
            let region = lines[idx + 1..]
                .iter()
                .take_while(|(_, l)| l.trim().is_empty() || l.starts_with([' ', '\t', '-']));
            let mut indent = None;
            let mut last_end = None;
            for &(off, l) in region {
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

/// Parse frontmatter YAML as a mapping; empty frontmatter is an empty mapping.
fn parse_mapping(yaml: &str) -> anyhow::Result<serde_yaml::Mapping> {
    match serde_yaml::from_str(yaml)? {
        serde_yaml::Value::Null => Ok(Default::default()),
        serde_yaml::Value::Mapping(m) => Ok(m),
        _ => anyhow::bail!("frontmatter is not a mapping"),
    }
}

/// `s` as a YAML scalar: plain when unambiguous, else double-quoted. Valid in
/// both block and flow context.
fn yaml_scalar(s: &str) -> String {
    let plain = s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '/' | '.'))
        && serde_yaml::from_str::<serde_yaml::Value>(s).ok()
            == Some(serde_yaml::Value::String(s.into()));
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

/// Byte layout of a note's leading frontmatter block.
struct Fenced {
    /// The YAML between the fences.
    yaml: std::ops::Range<usize>,
    /// Line ending of the opening fence (`\n` or `\r\n`).
    newline: &'static str,
}

/// Locate the frontmatter: an optional BOM, a first line that is exactly
/// `---`, and the next line that is exactly `---` (trailing whitespace
/// allowed on both). An indented `---` is YAML content, not a fence.
fn split_frontmatter(content: &str) -> Option<Fenced> {
    let start = if content.starts_with('\u{feff}') {
        '\u{feff}'.len_utf8()
    } else {
        0
    };
    let mut lines = content[start..].split_inclusive('\n');
    let open = lines.next()?;
    if !open.ends_with('\n') || open.trim_end() != "---" {
        return None;
    }
    let newline = if open.ends_with("\r\n") { "\r\n" } else { "\n" };
    let yaml_start = start + open.len();
    let mut pos = yaml_start;
    for line in lines {
        if line.trim_end() == "---" {
            return Some(Fenced {
                yaml: yaml_start..pos,
                newline,
            });
        }
        pos += line.len();
    }
    None
}

/// Slice the YAML between the frontmatter fences.
fn extract_yaml(content: &str) -> Option<&str> {
    split_frontmatter(content).map(|f| &content[f.yaml])
}

/// A string list read leniently: a single string or number becomes one item,
/// non-scalar list items are skipped, anything else is empty.
fn lenient_strings(v: &serde_yaml::Value) -> Vec<String> {
    use serde_yaml::Value;
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
    Ok(lenient_strings(&serde_yaml::Value::deserialize(d)?))
}

/// Strip a `sc-`/`it-`/`ep-` prefix and parse the numeric id.
fn parse_prefixed_id(s: &str) -> Option<i32> {
    s.rsplit('-').next()?.parse().ok()
}

/// Faithful mirror of on-disk YAML — every field optional, so any real note
/// (including plain Obsidian notes and legacy shapes) deserializes. Used for
/// both read and write; `skip_serializing_if` keeps written blocks tidy.
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

    // Story note
    #[serde(skip_serializing_if = "Option::is_none")]
    story_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    story_link: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    story_name: Option<String>,
    /// A story note's link to its iteration/epic.
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

    // Iteration note
    #[serde(skip_serializing_if = "Option::is_none")]
    iteration_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    iteration_link: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    iteration_name: Option<String>,

    // Epic note
    #[serde(skip_serializing_if = "Option::is_none")]
    epic_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    epic_link: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    epic_name: Option<String>,
}

impl From<RawFrontmatter> for Frontmatter {
    fn from(r: RawFrontmatter) -> Self {
        // Classify by which id is present. A note links to at most one entity.
        let link = if let Some(id) = r.story_id.as_deref().and_then(parse_prefixed_id) {
            EntityLink::Story {
                id,
                url: r.story_link.unwrap_or_default(),
                name: r.story_name,
                iteration_url: r.iteration,
                epic_url: r.epic,
            }
        } else if let Some(id) = r.iteration_id.as_deref().and_then(parse_prefixed_id) {
            EntityLink::Iteration {
                id,
                url: r.iteration_link.unwrap_or_default(),
                name: r.iteration_name,
            }
        } else if let Some(id) = r.epic_id.as_deref().and_then(parse_prefixed_id) {
            EntityLink::Epic {
                id,
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

    #[test]
    fn reads_story_note() {
        let content = "---\nid: my-story\naliases: []\ntags: [a]\ncreated: \"2026-03-31\"\niteration: https://it/1\nstory_id: sc-19559\nstory_link: https://s/19559\n---\n\nbody";
        let fm = Frontmatter::from_note(content).unwrap();
        assert_eq!(fm.id, "my-story");
        assert_eq!(fm.tags, vec!["a"]);
        assert!(matches!(fm.link, EntityLink::Story { id: 19559, .. }));
    }

    #[test]
    fn reads_bare_created_and_iteration_note() {
        let content = "---\niteration_id: it-17842\niteration_link: https://it/17842\ncreated: 2026-02-18\n---\n";
        let fm = Frontmatter::from_note(content).unwrap();
        assert_eq!(fm.created, NaiveDate::from_ymd_opt(2026, 2, 18));
        assert!(matches!(fm.link, EntityLink::Iteration { id: 17842, .. }));
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
            42,
            "Name".into(),
            "https://s/42".into(),
            Some("https://it/1".into()),
        );
        let block = fm.to_block().unwrap();
        let full = format!("{block}\nbody");
        let back = Frontmatter::from_note(&full).unwrap();
        assert!(matches!(back.link, EntityLink::Story { id: 42, .. }));
        assert!(block.contains("story_id: sc-42"));
    }

    #[test]
    fn branches_roundtrip_through_block() {
        let mut fm = Frontmatter::story("s".into(), 42, "Name".into(), "https://s/42".into(), None);
        assert!(!fm.to_block().unwrap().contains("branches"));
        fm.branches = vec!["me/sc-42/name".into()];
        let full = format!("{}\nbody", fm.to_block().unwrap());
        let back = Frontmatter::from_note(&full).unwrap();
        assert_eq!(back.branches, vec!["me/sc-42/name"]);
    }

    #[test]
    fn add_branch_is_idempotent_and_keeps_body() {
        let content = "---\nid: s\nstory_id: sc-42\n---\n\n# Title\n\nbody\n";
        let once = add_branch(content, "feat-x").unwrap();
        let twice = add_branch(&once, "feat-x").unwrap();
        assert_eq!(once, twice);
        assert!(once.ends_with("\n---\n\n# Title\n\nbody\n"));
        let fm = Frontmatter::from_note(&once).unwrap();
        assert_eq!(fm.branches, vec!["feat-x"]);
        assert!(matches!(fm.link, EntityLink::Story { id: 42, .. }));

        let more = add_branch(&once, "feat-y").unwrap();
        assert_eq!(
            Frontmatter::from_note(&more).unwrap().branches,
            vec!["feat-x", "feat-y"]
        );
    }

    #[test]
    fn add_branch_preserves_unknown_keys() {
        let content = "---\nid: s\ncssclasses: [wide]\npublish: true\nbranches:\n---\nbody";
        let out = add_branch(content, "b").unwrap();
        assert_eq!(
            out,
            "---\nid: s\ncssclasses: [wide]\npublish: true\nbranches:\n  - b\n---\nbody"
        );
        assert_eq!(Frontmatter::from_note(&out).unwrap().branches, vec!["b"]);
    }

    #[test]
    fn fences_are_whole_lines() {
        // Empty frontmatter, then a body containing `Note:` and a `---` rule.
        let content = "---\n---\n# Meeting\nNote: remember X\n\n---\nfooter\n";
        assert_eq!(extract_yaml(content), Some(""));
        let out = add_branch(content, "b").unwrap();
        assert_eq!(
            out,
            "---\nbranches:\n  - b\n---\n# Meeting\nNote: remember X\n\n---\nfooter\n"
        );

        // `----` and indented `---` don't close the block.
        let content = "---\nid: a\nnote: |\n  ---\n  x\n----\n---\nbody";
        assert_eq!(
            extract_yaml(content),
            Some("id: a\nnote: |\n  ---\n  x\n----\n")
        );
        assert!(extract_yaml("---\nid: a\n").is_none());
        assert!(extract_yaml("--- \nid: a\n---  \n").is_some());
        assert!(extract_yaml("x\n---\nid: a\n---\n").is_none());
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
        assert!(matches!(fm.link, EntityLink::Story { id: 3, .. }));
    }

    #[test]
    fn add_branch_leaves_other_bytes_alone() {
        let head = "---\n# arc metadata\nid: s # slug\ncreated: \"2026-03-31\"\nversion: 1.10\nsummary: >\n  folded\n  text\n";
        let body = "---\n\n# Title\n\n---\nbody with trailing spaces  \n";
        let out = add_branch(&format!("{head}{body}"), "feat/x").unwrap();
        assert_eq!(out, format!("{head}branches:\n  - feat/x\n{body}"));
    }

    #[test]
    fn add_branch_appends_to_block_list() {
        let content = "---\nid: s\nbranches:\n    - a # first\n    - b\n\ntags: [t]\n---\nbody";
        assert_eq!(
            add_branch(content, "c").unwrap(),
            "---\nid: s\nbranches:\n    - a # first\n    - b\n    - c\n\ntags: [t]\n---\nbody"
        );
        let content = "---\nbranches:\n- a\n---\n";
        assert_eq!(
            add_branch(content, "c").unwrap(),
            "---\nbranches:\n- a\n- c\n---\n"
        );
    }

    #[test]
    fn add_branch_flow_empty_and_scalar() {
        assert_eq!(
            add_branch("---\nbranches: [a, 'b']\nx: 1\n---\n", "c").unwrap(),
            "---\nbranches: [a, 'b', c]\nx: 1\n---\n"
        );
        assert_eq!(
            add_branch("---\nbranches: []\nx: 1\n---\n", "c").unwrap(),
            "---\nbranches:\n  - c\nx: 1\n---\n"
        );
        assert_eq!(
            add_branch("---\nbranches: ~\n---\n", "c").unwrap(),
            "---\nbranches:\n  - c\n---\n"
        );
        assert_eq!(
            add_branch("---\nbranches: main\nx: 1\n---\n", "c").unwrap(),
            "---\nbranches:\n  - main\n  - c\nx: 1\n---\n"
        );
        // Already listed as a scalar: no-op.
        let content = "---\nbranches: main\n---\n";
        assert_eq!(add_branch(content, "main").unwrap(), content);
    }

    #[test]
    fn add_branch_crlf_and_bom() {
        let content = "\u{feff}---\r\nid: s\r\nbranches:\r\n  - a\r\n---\r\nbody\r\n";
        let out = add_branch(content, "b").unwrap();
        assert_eq!(
            out,
            "\u{feff}---\r\nid: s\r\nbranches:\r\n  - a\r\n  - b\r\n---\r\nbody\r\n"
        );
        assert_eq!(add_branch(&out, "b").unwrap(), out);

        let out = add_branch("---\r\nid: s\r\n---\r\n", "b").unwrap();
        assert_eq!(out, "---\r\nid: s\r\nbranches:\r\n  - b\r\n---\r\n");
    }

    #[test]
    fn add_branch_quotes_ambiguous_names() {
        let out = add_branch("---\nid: s\n---\n", "true").unwrap();
        assert!(out.contains("  - \"true\"\n"));
        let out = add_branch(&out, "a: b#c").unwrap();
        assert!(out.contains("  - \"a: b#c\"\n"));
        assert_eq!(
            Frontmatter::from_note(&out).unwrap().branches,
            vec!["true", "a: b#c"]
        );
        let out = add_branch("---\nbranches: [a]\n---\n", "x,y").unwrap();
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
            assert!(add_branch(content, "z").is_err(), "{content:?}");
        }
        assert!(add_branch("---\nid: s\n---\n", "").is_err());
    }

    #[test]
    fn add_branch_requires_frontmatter() {
        assert!(add_branch("# no fence\n", "b").is_err());
        assert!(add_branch("---\nid: s\n", "b").is_err());
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
            7,
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
        let fm = Frontmatter::story("sc-8-slug".into(), 8, "Story eight".into(), "https://s/8".into(), None);
        assert_eq!(
            pinned_block(fm),
            "---\nid: sc-8-slug\ncreated: 2026-03-31\nstory_id: sc-8\nstory_link: https://s/8\nstory_name: Story eight\n---\n"
        );
    }

    #[test]
    fn golden_iteration_block() {
        let fm = Frontmatter::iteration("it-3-slug".into(), 3, "Sprint 3".into(), "https://it/3".into());
        assert_eq!(
            pinned_block(fm),
            "---\nid: it-3-slug\ncreated: 2026-03-31\niteration_id: it-3\niteration_link: https://it/3\niteration_name: Sprint 3\n---\n"
        );
    }

    #[test]
    fn golden_epic_block() {
        let fm = Frontmatter::epic("ep-40-slug".into(), 40, "Epic forty".into(), "https://ep/40".into());
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
                id: 9,
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
