//! Conflict ignore rules (monolith `analysisConfig.conflictIgnorePaths`). @sat:ignore
//!
//! Owns: the user's list of paths that are no conflict, their normal form, and matching a path against them.
//! Not here: leaving matched paths out of the conflict index (`@sat:overwrite.conflicts`, `build_with`), keeping
//! the list in the project (`@sat:project`).
//!
//! Three forms, as in the monolith (`isConflictPathIgnored`, `08-conflicts.md` §4.1), on case-folded paths with `/`:
//! - `meta.ini` (no `/`) — a file of that name anywhere: `meta.ini`, `x/y/meta.ini`;
//! - `docs/` (trailing `/`) — a folder of that name anywhere: `docs/a.txt`, `fomod/docs/b.png`;
//! - `textures/sky/a.dds` — exactly that path.
//!
//! No glob, like the monolith: the list stays readable and cheap to check (only contested paths are checked).

use serde::{Deserialize, Serialize};

/// The rules, normalized and sorted (the project keeps them in this form).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IgnoreRules(Vec<String>);

/// The normal form of a rule or path: case-folded, `\` → `/`, no leading `/` or `selected/` (monolith).
pub fn normalize(p: &str) -> String {
    let p = p.trim().replace('\\', "/").to_lowercase();
    let p = p.trim_start_matches('/');
    let p = p.strip_prefix("selected/").unwrap_or(p);
    let mut out = String::with_capacity(p.len());
    for part in p.split('/') {
        if part.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push('/');
        }
        out.push_str(part);
    }
    if p.ends_with('/') && !out.is_empty() {
        out.push('/');
    }
    out
}

impl IgnoreRules {
    pub fn new(rules: impl IntoIterator<Item = impl AsRef<str>>) -> Self {
        let mut v: Vec<String> = rules
            .into_iter()
            .map(|r| normalize(r.as_ref()))
            .filter(|r| !r.is_empty())
            .collect();
        v.sort();
        v.dedup();
        Self(v)
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn rules(&self) -> &[String] {
        &self.0
    }

    // @sat:ignore.matches
    /// Whether a path is ignored.
    pub fn matches(&self, path: &str) -> bool {
        let p = normalize(path);
        self.0.iter().any(|r| {
            if let Some(dir) = r.strip_suffix('/') {
                p.starts_with(r) || p.contains(&format!("/{dir}/"))
            } else if !r.contains('/') {
                p == *r || p.ends_with(&format!("/{r}"))
            } else {
                p == *r
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overwrite::{ConflictIndex, OverwriteProjection, Priorities};
    use crate::table::ProviderTable;
    use crate::table::tests::mk;

    #[test]
    fn the_three_forms() {
        let r = IgnoreRules::new(["Meta.INI", "\\docs\\", "textures/Sky/a.dds", ""]);
        assert_eq!(r.rules(), ["docs/", "meta.ini", "textures/sky/a.dds"]);
        for yes in [
            "meta.ini",
            "a/b/META.ini",
            "docs/x.txt",
            "fomod/docs/y.png",
            "Textures\\sky\\A.dds",
            "selected/meta.ini",
        ] {
            assert!(r.matches(yes), "{yes}");
        }
        for no in [
            "meta.ini.bak",
            "xmeta.ini",
            "mydocs/x.txt",
            "textures/sky/b.dds",
            "textures/a.dds",
        ] {
            assert!(!r.matches(no), "{no}");
        }
    }

    #[test]
    fn ignored_paths_are_no_conflict() {
        let t = ProviderTable::build(&[
            mk("A", &["readme.txt", "x.dds"]),
            mk("B", &["readme.txt", "x.dds"]),
        ]);
        let full = OverwriteProjection::compute(&t, Priorities(vec![0, 1]));
        assert_eq!(full.totals().conflict_paths, 2);
        let idx = ConflictIndex::build_with(&t, &IgnoreRules::new(["readme.txt"]));
        assert_eq!((idx.contested_count(), idx.ignored_count()), (1, 1));
        let p = OverwriteProjection::compute_with(std::sync::Arc::new(idx), &t, Priorities(vec![0, 1]));
        assert_eq!(p.totals().conflict_paths, 1);
    }
}
