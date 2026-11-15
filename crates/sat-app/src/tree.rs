//! A mod's file tree with the conflict overlay (monolith tree overlay, `08-conflicts.md` §4.8, 82 930). @sat:app.tree
//!
//! Owns: one folder level of a mod's virtual files (loose and archive entries, each path once) with, per entry, its
//! files and how many of them the mod wins, loses, or owns while disabled — a folder sums everything under it.
//! Not here: the overwrite (`@sat:overwrite`), drawing (the page: a row that loses anything is red before green, as
//! in the monolith — losing matters more).
//!
//! The monolith marks a path won when the mod is its winner and lost when another mod is, and spreads the state to
//! every folder above; here the counts carry the same and the page picks the colour.

use serde::{Deserialize, Serialize};

use sat_core::ids::ModId;
use sat_core::order::Key;

use crate::App;

/// One child of a folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeEntry {
    pub name: String,
    pub dir: bool,
    pub files: u32,
    pub wins: u32,
    pub loses: u32,
    /// Contested paths it has while disabled.
    #[serde(default)]
    pub owned: u32,
}

impl App {
    /// The children of folder `dir` (`""` = the root, else `a/b`) of mod `key`: folders first, then files.
    pub(crate) fn mod_tree(&mut self, key: Key, dir: &str) -> Option<Vec<TreeEntry>> {
        let ow = self.session.overwrite();
        let table = self.session.table().clone();
        let name = self.session.book().name(key).to_lowercase();
        let m = table
            .mod_names()
            .iter()
            .position(|n| n.to_lowercase() == name)
            .map(ModId::from_idx)?;
        let unit = ow.units().rep(m);
        let active = ow.priorities().is_active(m);
        let prefix = if dir.is_empty() {
            String::new()
        } else {
            format!("{}/", dir.trim_matches('/').to_lowercase())
        };
        let mut paths: Vec<sat_core::ids::PathId> = table
            .mod_providers(m)
            .iter()
            .map(|&i| table.path_of_provider(i as usize))
            .collect();
        paths.sort_unstable();
        paths.dedup();
        let mut by: std::collections::BTreeMap<(bool, String), TreeEntry> = Default::default();
        for p in paths {
            let s = table.path_string(p);
            let n = prefix.len();
            if s.len() <= n || !s.is_char_boundary(n) || s[..n].to_lowercase() != prefix {
                continue;
            }
            let rest = &s[n..];
            let (child, is_dir) = match rest.split_once('/') {
                Some((d, _)) => (d.to_string(), true),
                None => (rest.to_string(), false),
            };
            let e = by
                .entry((!is_dir, child.to_lowercase()))
                .or_insert_with(|| TreeEntry {
                    name: child,
                    dir: is_dir,
                    files: 0,
                    wins: 0,
                    loses: 0,
                    owned: 0,
                });
            e.files += 1;
            match ow.path_chain(p).map(|c| ow.chain_mods(c)) {
                Some(st) if st.first() == Some(&unit) => e.wins += 1,
                Some(st) if st.contains(&unit) => e.loses += 1,
                Some(_) if !active => e.owned += 1,
                _ => {}
            }
        }
        Some(by.into_values().collect())
    }
}

#[cfg(test)]
mod tests {
    use crate::{Query, Reply};

    #[test]
    fn folders_sum_what_the_mod_wins_and_loses_under_them() {
        let mut a = crate::tests::app_of(["A", "B"]);
        let tree = |a: &mut crate::App, n: &str, dir: &str| {
            let key = a.session().book().key_of(n).unwrap();
            let Reply::ModTree { entries } = a.query(Query::ModTree { key, dir: dir.into() }).unwrap() else {
                panic!()
            };
            entries
                .into_iter()
                .map(|e| (e.name, e.dir, e.files, e.wins, e.loses))
                .collect::<Vec<_>>()
        };
        assert_eq!(tree(&mut a, "A", ""), [("textures".to_string(), true, 1, 0, 1)]);
        assert_eq!(
            tree(&mut a, "B", "textures"),
            [("a.dds".to_string(), false, 1, 1, 0)]
        );
        assert_eq!(
            tree(&mut a, "B", "Textures/"),
            [("a.dds".to_string(), false, 1, 1, 0)],
            "case and slashes"
        );
    }
}
