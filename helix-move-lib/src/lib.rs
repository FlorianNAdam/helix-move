use std::{
    collections::BTreeSet,
    fmt::Display,
    fs,
    path::{Path, PathBuf},
};

#[cfg(unix)]
use std::os::unix::ffi::{OsStrExt, OsStringExt};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct EntryPath {
    path: PathBuf,
    is_dir: bool,
}

impl EntryPath {
    pub fn new(path: PathBuf, is_dir: bool) -> Self {
        Self { path, is_dir }
    }

    pub fn from_editor_line(line: &str) -> Self {
        let is_dir = line.ends_with('/');
        let path = parse_editor_path(line.trim_end_matches('/'));
        Self { path, is_dir }
    }

    pub fn to_editor_line(&self) -> String {
        let mut line = format_editor_path(&self.path);
        if self.is_dir {
            line.push('/');
        }
        line
    }

    pub fn as_path(&self) -> &Path {
        &self.path
    }
}

impl Display for EntryPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_editor_line())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rule {
    Unchanged { path: EntryPath },
    Renamed { from: EntryPath, to: EntryPath },
    Deleted { path: EntryPath },
}

impl Into<FullRule> for &Rule {
    fn into(self) -> FullRule {
        match self {
            Rule::Unchanged { path } => FullRule::Unchanged {
                path: path.clone(),
            },
            Rule::Renamed { from, to } => FullRule::Renamed {
                from: from.clone(),
                to: to.clone(),
            },
            Rule::Deleted { path } => FullRule::Deleted { path: path.clone() },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FullRule {
    Unchanged { path: EntryPath },
    Renamed { from: EntryPath, to: EntryPath },
    Deleted { path: EntryPath },
    Added { path: EntryPath },
}

impl Into<Option<EditRule>> for &FullRule {
    fn into(self) -> Option<EditRule> {
        match self {
            FullRule::Unchanged { path } => Some(EditRule::Unchanged {
                path: path.clone(),
            }),
            FullRule::Renamed { from, to } => Some(EditRule::Renamed {
                from: from.clone(),
                to: to.clone(),
            }),
            FullRule::Deleted { path: _ } => None,
            FullRule::Added { path } => Some(EditRule::Added { path: path.clone() }),
        }
    }
}

impl Display for FullRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FullRule::Unchanged { path } => write!(f, "! {path}"),
            FullRule::Renamed { from, to } => write!(f, "~ {from} -> {to}"),
            FullRule::Deleted { path } => write!(f, "- {path}"),
            FullRule::Added { path } => write!(f, "+ {path}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditRule {
    Unchanged { path: EntryPath },
    Renamed { from: EntryPath, to: EntryPath },
    Added { path: EntryPath },
}

impl EditRule {
    pub fn apply(&self, old_root: &Path, new_root: &Path) -> anyhow::Result<()> {
        let old_root = old_root.canonicalize()?;
        let new_root = new_root.canonicalize()?;

        match self {
            EditRule::Unchanged { path: raw_path } => {
                let old_path = old_root.join(raw_path.as_path());
                let new_path = new_root.join(raw_path.as_path());

                assert!(old_path.exists());
                assert!(old_path.is_dir() == raw_path.is_dir);

                if old_path.is_dir() {
                    fs::create_dir(new_path)?;
                } else {
                    fs::copy(old_path, new_path)?;
                }
            }
            EditRule::Renamed {
                from: raw_from,
                to: raw_to,
            } => {
                let from = old_root.join(raw_from.as_path());
                let to = new_root.join(raw_to.as_path());

                assert!(from.exists());
                assert!(raw_from.is_dir == raw_to.is_dir);
                assert!(from.is_dir() == raw_from.is_dir);

                if from.is_dir() {
                    fs::create_dir(to)?;
                } else {
                    fs::copy(from, to)?;
                }
            }
            EditRule::Added { path: raw_path } => {
                let new_path = new_root.join(raw_path.as_path());

                assert!(raw_path.is_dir);

                fs::create_dir(new_path)?;
            }
        }

        Ok(())
    }
}

pub fn build_rules(original: &[String], new: &[String]) -> Vec<Rule> {
    assert_eq!(original.len(), new.len(), "original and new must have same length");

    original
        .iter()
        .zip(new.iter())
        .map(|(old, new)| {
            if old == new {
                Rule::Unchanged {
                    path: EntryPath::from_editor_line(old),
                }
            } else if new.starts_with("- ") && !old.starts_with("- ") {
                Rule::Deleted {
                    path: EntryPath::from_editor_line(old),
                }
            } else {
                Rule::Renamed {
                    from: EntryPath::from_editor_line(old),
                    to: EntryPath::from_editor_line(new),
                }
            }
        })
        .collect()
}

pub fn normalize_rules(rules: &[Rule]) -> Vec<Rule> {
    let delete_paths: Vec<EntryPath> = rules
        .iter()
        .filter_map(|r| {
            if let Rule::Deleted { path } = r {
                Some(path.clone())
            } else {
                None
            }
        })
        .collect();

    let stage1: Vec<Rule> = rules
        .iter()
        .map(|rule| apply_deletes(rule, &delete_paths))
        .collect();

    let mut indices: Vec<usize> = (0..stage1.len()).collect();
    indices.sort_by_key(|&i| rule_depth(&stage1[i]));

    let mut normalized = stage1.clone();

    for &i in &indices {
        let current = normalized[i].clone();

        if let Rule::Renamed { from, to } = &current {
            for j in (i + 1)..normalized.len() {
                normalized[j] = rewrite_rule(&normalized[j], from, to);
            }
        }
    }

    normalized
}

pub fn add_missing_directories(rules: &[Rule]) -> Vec<FullRule> {
    let mut full_rules = Vec::new();
    let mut existing: BTreeSet<EntryPath> = rules.iter().filter_map(rule_dest_path).collect();

    for rule in rules {
        match rule {
            Rule::Renamed { from, to } => {
                let missing = get_missing_parents(to, &existing);

                for parent in missing {
                    full_rules.push(FullRule::Added { path: parent.clone() });
                    existing.insert(parent);
                }

                full_rules.push(FullRule::Renamed {
                    from: from.clone(),
                    to: to.clone(),
                });
                existing.insert(to.clone());
            }

            Rule::Unchanged { path } => {
                full_rules.push(FullRule::Unchanged { path: path.clone() });
                existing.insert(path.clone());
            }

            Rule::Deleted { path } => {
                full_rules.push(FullRule::Deleted { path: path.clone() });
            }
        }
    }

    full_rules
}

pub fn filter_full_rules(rules: &[FullRule]) -> Vec<FullRule> {
    rules
        .iter()
        .filter(|r| !matches!(r, FullRule::Unchanged { .. }))
        .cloned()
        .collect()
}

pub fn create_edit_rules(rules: &[FullRule]) -> Vec<EditRule> {
    rules.iter().filter_map(|r| r.into()).collect()
}

fn get_missing_parents(path: &EntryPath, existing: &BTreeSet<EntryPath>) -> Vec<EntryPath> {
    let mut missing = Vec::new();
    let mut current = path.clone();

    while let Some(parent) = parent_dir(&current) {
        if !existing.contains(&parent) {
            missing.push(parent.clone());
        }
        current = parent;
    }

    missing.reverse();
    missing
}

fn parent_dir(path: &EntryPath) -> Option<EntryPath> {
    path.path.parent().and_then(|parent| {
        if parent.as_os_str().is_empty() {
            None
        } else {
            Some(EntryPath::new(parent.to_path_buf(), true))
        }
    })
}

pub fn apply_rules_to_list(rules: &[Rule]) -> Vec<String> {
    let mut result = Vec::new();

    for rule in rules {
        match rule {
            Rule::Unchanged { path } => result.push(path.to_editor_line()),
            Rule::Renamed { to, .. } => result.push(to.to_editor_line()),
            Rule::Deleted { .. } => result.push("- ".to_string()),
        }
    }

    result
}

fn apply_deletes(rule: &Rule, deletes: &[EntryPath]) -> Rule {
    match rule {
        Rule::Deleted { path } => Rule::Deleted { path: path.clone() },

        Rule::Unchanged { path } => {
            if deletes.iter().any(|d| path_starts_with(path, d)) {
                Rule::Deleted { path: path.clone() }
            } else {
                Rule::Unchanged { path: path.clone() }
            }
        }

        Rule::Renamed { from, to } => {
            if deletes.iter().any(|d| path_starts_with(from, d)) {
                Rule::Deleted { path: from.clone() }
            } else {
                Rule::Renamed {
                    from: from.clone(),
                    to: to.clone(),
                }
            }
        }
    }
}

fn rewrite_rule(rule: &Rule, from: &EntryPath, to: &EntryPath) -> Rule {
    match rule {
        Rule::Deleted { path } => Rule::Deleted { path: path.clone() },

        Rule::Unchanged { path } => {
            let new_path = rewrite_path(path, from, to);

            if &new_path == path {
                Rule::Unchanged { path: path.clone() }
            } else {
                Rule::Renamed {
                    from: path.clone(),
                    to: new_path,
                }
            }
        }

        Rule::Renamed { from: f, to: t } => Rule::Renamed {
            from: f.clone(),
            to: rewrite_path(t, from, to),
        },
    }
}

fn rewrite_path(path: &EntryPath, from: &EntryPath, to: &EntryPath) -> EntryPath {
    if !path_starts_with(path, from) || path.path == from.path {
        return path.clone();
    }

    let suffix = path.path.strip_prefix(&from.path).unwrap();
    EntryPath::new(to.path.join(suffix), path.is_dir)
}

fn path_starts_with(path: &EntryPath, base: &EntryPath) -> bool {
    path.path.starts_with(&base.path)
}

fn depth(path: &EntryPath) -> usize {
    path.path.components().count()
}

fn rule_depth(rule: &Rule) -> usize {
    match rule {
        Rule::Unchanged { path } => depth(path),
        Rule::Renamed { from, .. } => depth(from),
        Rule::Deleted { path } => depth(path),
    }
}

fn rule_dest_path(rule: &Rule) -> Option<EntryPath> {
    match rule {
        Rule::Deleted { .. } => None,
        Rule::Unchanged { path } => Some(path.clone()),
        Rule::Renamed { to, .. } => Some(to.clone()),
    }
}

pub fn format_editor_path(path: &Path) -> String {
    #[cfg(unix)]
    {
        let mut result = String::new();
        for &byte in path.as_os_str().as_bytes() {
            if byte == b'/' || (byte.is_ascii_graphic() && byte != b'%') || byte == b' ' {
                result.push(byte as char);
            } else {
                result.push_str(&format!("%{byte:02X}"));
            }
        }
        result
    }

    #[cfg(not(unix))]
    {
        path.to_string_lossy().into_owned()
    }
}

pub fn parse_editor_path(path: &str) -> PathBuf {
    #[cfg(unix)]
    {
        let bytes = path.as_bytes();
        let mut decoded = Vec::with_capacity(bytes.len());
        let mut i = 0;

        while i < bytes.len() {
            if bytes[i] == b'%' && i + 2 < bytes.len() {
                if let (Some(high), Some(low)) =
                    (hex_value(bytes[i + 1]), hex_value(bytes[i + 2]))
                {
                    decoded.push((high << 4) | low);
                    i += 3;
                    continue;
                }
            }

            decoded.push(bytes[i]);
            i += 1;
        }

        std::ffi::OsString::from_vec(decoded).into()
    }

    #[cfg(not(unix))]
    {
        PathBuf::from(path)
    }
}

#[cfg(unix)]
fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn editor_path_encoding_round_trips_invalid_utf8() {
        let path = PathBuf::from(std::ffi::OsString::from_vec(vec![
            b'S', b'l', b'i', b'd', b'e', b's', b' ', 0xD4, 0xC7, 0xEF, b'.', b'p', b'd', b'f',
        ]));

        let encoded = format_editor_path(&path);

        assert_eq!(encoded, "Slides %D4%C7%EF.pdf");
        assert_eq!(parse_editor_path(&encoded), path);
    }
}
