use std::{collections::BTreeSet, fmt::Display, fs, path::Path};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rule {
    Unchanged { path: String },
    Renamed { from: String, to: String },
    Deleted { path: String },
}

impl Into<FullRule> for &Rule {
    fn into(self) -> FullRule {
        match self {
            Rule::Unchanged { path } => FullRule::Unchanged {
                path: path.to_string(),
            },
            Rule::Renamed { from, to } => FullRule::Renamed {
                from: from.to_string(),
                to: to.to_string(),
            },
            Rule::Deleted { path } => FullRule::Deleted {
                path: path.to_string(),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FullRule {
    Unchanged { path: String },
    Renamed { from: String, to: String },
    Deleted { path: String },
    Added { path: String },
}

impl Into<Option<EditRule>> for &FullRule {
    fn into(self) -> Option<EditRule> {
        match self {
            FullRule::Unchanged { path } => Some(EditRule::Unchanged {
                path: path.to_string(),
            }),
            FullRule::Renamed { from, to } => Some(EditRule::Renamed {
                from: from.to_string(),
                to: to.to_string(),
            }),
            FullRule::Deleted { path: _ } => None,
            FullRule::Added { path } => Some(EditRule::Added {
                path: path.to_string(),
            }),
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
    Unchanged { path: String },
    Renamed { from: String, to: String },
    Added { path: String },
}

impl EditRule {
    pub fn apply(
        &self,
        old_root: &Path,
        new_root: &Path,
    ) -> anyhow::Result<()> {
        let old_root = old_root.canonicalize()?;
        let new_root = new_root.canonicalize()?;

        match self {
            EditRule::Unchanged { path: raw_path } => {
                let old_path = old_root.join(raw_path);
                let new_path = new_root.join(raw_path);

                assert!(old_path.exists());
                assert!(old_path.is_dir() == raw_path.ends_with("/"));

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
                let from = old_root.join(&raw_from);
                let to = new_root.join(&raw_to);

                assert!(from.exists());
                assert!(raw_from.ends_with("/") == raw_to.ends_with("/"));
                assert!(from.is_dir() == raw_from.ends_with("/"));

                if from.is_dir() {
                    fs::create_dir(to)?;
                } else {
                    fs::copy(from, to)?;
                }
            }
            EditRule::Added { path: raw_path } => {
                let new_path = new_root.join(&raw_path);

                assert!(raw_path.ends_with("/"));

                fs::create_dir(new_path)?;
            }
        }

        Ok(())
    }
}

//
// -----------------------------
// Phase 1 — Build Rules
// -----------------------------
//
pub fn build_rules(original: &[String], new: &[String]) -> Vec<Rule> {
    assert_eq!(
        original.len(),
        new.len(),
        "original and new must have same length"
    );

    original
        .iter()
        .zip(new.iter())
        .map(|(old, new)| {
            if old == new {
                Rule::Unchanged { path: old.clone() }
            } else if new.starts_with("- ") && !old.starts_with("- ") {
                Rule::Deleted { path: old.clone() }
            } else {
                Rule::Renamed {
                    from: old.clone(),
                    to: new.clone(),
                }
            }
        })
        .collect()
}

//
// -----------------------------
// Phase 2 — Normalize Rules
// -----------------------------
//
pub fn normalize_rules(rules: &[Rule]) -> Vec<Rule> {
    // ---- STEP 1: APPLY DELETES ----

    let delete_paths: Vec<String> = rules
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

    // ---- STEP 2: APPLY RENAMES ----

    // depth-sort indices (shallow first)
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

//
// -----------------------------
// Phase 3 — Detect new directories
// -----------------------------
//

pub fn add_missing_directories(rules: &[Rule]) -> Vec<FullRule> {
    let mut full_rules = Vec::new();

    // 1️⃣ Collect existing destination paths
    let mut existing: BTreeSet<String> = rules
        .iter()
        .filter_map(rule_dest_path)
        .collect();

    for rule in rules {
        match rule {
            Rule::Renamed { from, to } => {
                // 2️⃣ Compute missing parents
                let missing = get_missing_parents(to, &existing);

                // 3️⃣ Add them first
                for parent in missing {
                    full_rules.push(FullRule::Added {
                        path: parent.clone(),
                    });
                    existing.insert(parent);
                }

                // 4️⃣ Push rename
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
        .map(|r| r.clone())
        .collect()
}

//
// -----------------------------
// Phase 4 — Create edit rules
// -----------------------------
//

pub fn create_edit_rules(rules: &[FullRule]) -> Vec<EditRule> {
    let edit_rules: Vec<EditRule> = rules
        .iter()
        .filter_map(|r| r.into())
        .collect();

    edit_rules
}

fn get_missing_parents(path: &str, existing: &BTreeSet<String>) -> Vec<String> {
    let mut missing = Vec::new();
    let mut current = path.to_string();

    while let Some(parent) = parent_dir(&current) {
        if !existing.contains(&parent) {
            missing.push(parent.clone());
        }
        current = parent;
    }

    missing.reverse();
    missing
}

fn parent_dir(path: &str) -> Option<String> {
    let trimmed = path.trim_end_matches('/');

    if let Some(pos) = trimmed.rfind('/') {
        if pos == 0 {
            // would become "/" — stop
            return None;
        }

        let parent = format!("{}/", &trimmed[..pos]);

        // stop at logical roots
        if parent == "/" || parent == "./" {
            None
        } else {
            Some(parent)
        }
    } else {
        None
    }
}

pub fn apply_rules_to_list(rules: &[Rule]) -> Vec<String> {
    let mut result = Vec::new();

    for rule in rules {
        match rule {
            Rule::Unchanged { path } => {
                result.push(path.clone());
            }

            Rule::Renamed { to, .. } => {
                result.push(to.clone());
            }

            Rule::Deleted { .. } => {
                result.push("- ".to_string());
            }
        }
    }

    result
}

//
// -----------------------------
// Rule Application Logic
// -----------------------------
//
fn apply_deletes(rule: &Rule, deletes: &[String]) -> Rule {
    match rule {
        Rule::Deleted { path } => Rule::Deleted { path: path.clone() },

        Rule::Unchanged { path } => {
            if deletes
                .iter()
                .any(|d| path.starts_with(d))
            {
                Rule::Deleted { path: path.clone() }
            } else {
                Rule::Unchanged { path: path.clone() }
            }
        }

        Rule::Renamed { from, to } => {
            if deletes
                .iter()
                .any(|d| from.starts_with(d))
            {
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

fn rewrite_rule(rule: &Rule, from: &str, to: &str) -> Rule {
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

fn rewrite_path(path: &str, from: &str, to: &str) -> String {
    if !path_starts_with(path, from) {
        return path.to_string();
    }

    if path == from {
        return path.to_string();
    }

    let path_parts: Vec<&str> = path
        .trim_end_matches('/')
        .split('/')
        .collect();

    let from_parts: Vec<&str> = from
        .trim_end_matches('/')
        .split('/')
        .collect();

    let to_parts: Vec<&str> = to
        .trim_end_matches('/')
        .split('/')
        .collect();

    // Replace prefix segments
    let mut new_parts = to_parts.clone();
    new_parts.extend_from_slice(&path_parts[from_parts.len()..]);

    let mut result = new_parts.join("/");

    // Preserve trailing slash if original had one
    if path.ends_with('/') {
        result.push('/');
    }

    result
}

fn path_starts_with(path: &str, base: &str) -> bool {
    let path_parts: Vec<&str> = path
        .trim_end_matches('/')
        .split('/')
        .collect();

    let base_parts: Vec<&str> = base
        .trim_end_matches('/')
        .split('/')
        .collect();

    if base_parts.len() > path_parts.len() {
        return false;
    }

    path_parts[..base_parts.len()] == base_parts[..]
}

fn depth(path: &str) -> usize {
    path.matches('/').count()
}

fn rule_depth(rule: &Rule) -> usize {
    match rule {
        Rule::Unchanged { path } => depth(path),
        Rule::Renamed { from, .. } => depth(from),
        Rule::Deleted { path } => depth(path),
    }
}

fn rule_dest_path(rule: &Rule) -> Option<String> {
    match rule {
        Rule::Deleted { .. } => None,
        Rule::Unchanged { path } => Some(path.clone()),
        Rule::Renamed { to, .. } => Some(to.clone()),
    }
}
