//! Factories: a logical group that a commit turns into one output mod (ADR 0014 §5, ADR 0016 roles). @sat:factory
//!
//! Owns: a factory's definition in the project (which group, the output mod, how), the fingerprint of its inputs
//! (the members and their content signatures) and whether it is dirty, and what a commit does for it: rebuild the
//! output beside the members, put it right after the group, switch the members off (they stay, reversible).
//! Not here: building the files (`@sat:mo2.bake`), the commit (`@sat:mo2.commit`), groups (`@sat:order.book`).
//!
//! The first kind is **bake**: the group's resulting files (the last member of a path wins, as in the game) put in
//! one folder — hard links when asked (no extra space on one volume), copies otherwise. Fewer mods for the
//! manager's virtual file system, one place to pack into an archive later.

use serde::{Deserialize, Serialize};

use crate::canon::{Entry, EntryKind, Modlist};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FactoryKind {
    /// The group's resulting files in one output mod.
    #[default]
    Bake,
}

// @sat:factory.mode
/// The plugin a packed output loads its archives with: the output's name, made a valid file name.
pub fn pack_plugin(output: &str) -> String {
    let stem: String = output
        .chars()
        .map(|c| if "\\/:*?\"<>|".contains(c) { '_' } else { c })
        .collect();
    format!("{}.esp", stem.trim())
}

/// The two roles of PHILOSOPHY «Лог-группа».
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FactoryMode {
    /// A living group: rebuilt whenever its inputs change; the members stay.
    #[default]
    Live,
    /// A finished group baked once and sealed: the members leave the build (into `deleted/`), the group and the
    /// factory are done. Not undone by the next commit — the commit asks for an explicit confirmation.
    Seal,
}

// @sat:factory.def
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Factory {
    /// The logical group (its stable id, `GroupDef::id`).
    pub group: String,
    /// The output mod (a folder in mods, an entry of the canon).
    pub output: String,
    #[serde(default)]
    pub kind: FactoryKind,
    /// Hard links instead of copies.
    #[serde(default)]
    pub link: bool,
    /// Pack the game's data folders into archives with an empty light plugin (`<output>.esp`, `.bsa`,
    /// ` - Textures.bsa`): thousands of loose files become a few (`@sat:formats.bsa-write`).
    #[serde(default)]
    pub pack: bool,
    /// Keep the members enabled next to the output (by default they are switched off: the output replaces them).
    #[serde(default)]
    pub keep_members: bool,
    /// What happens to the group after the build.
    #[serde(default)]
    pub mode: FactoryMode,
    /// Fingerprint of the inputs at the last build; `None` — never built.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub built: Option<String>,
}

// @sat:factory.fingerprint
/// Fingerprint of a factory's inputs: the members in order with their content signatures, and the settings.
pub fn fingerprint(f: &Factory, members: &[(String, Option<u64>)]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |b: &[u8]| {
        for &x in b {
            h = (h ^ u64::from(x)).wrapping_mul(0x0100_0000_01b3);
        }
        h = (h ^ 0xff).wrapping_mul(0x0100_0000_01b3);
    };
    eat(&[f.kind as u8, u8::from(f.link), u8::from(f.pack)]);
    for (name, sig) in members {
        eat(name.to_lowercase().as_bytes());
        eat(&sig.unwrap_or(0).to_le_bytes());
    }
    format!("{h:016x}")
}

// @sat:factory.canon
/// The canon after a build: the output right after the group's last member (enabled), the members off unless
/// kept. `members` in canon order.
pub fn place_output(canon: &Modlist, f: &Factory, members: &[String]) -> Modlist {
    let mut entries: Vec<Entry> = canon
        .entries
        .iter()
        .filter(|e| !e.name.eq_ignore_ascii_case(&f.output))
        .cloned()
        .collect();
    let is_member = |n: &str| members.iter().any(|m| m.eq_ignore_ascii_case(n));
    let last = entries
        .iter()
        .rposition(|e| is_member(&e.name))
        .map_or(entries.len(), |i| i + 1);
    entries.insert(
        last,
        Entry {
            name: f.output.clone(),
            kind: EntryKind::Mod,
            enabled: true,
        },
    );
    if !f.keep_members {
        for e in entries.iter_mut().filter(|e| is_member(&e.name)) {
            e.enabled = false;
        }
    }
    Modlist::new(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_output_follows_the_group_and_the_fingerprint_follows_the_inputs() {
        let canon = Modlist::new(
            ["A", "B", "C"]
                .iter()
                .map(|n| Entry {
                    name: n.to_string(),
                    kind: EntryKind::Mod,
                    enabled: true,
                })
                .collect(),
        );
        let f = Factory {
            group: "g1".into(),
            output: "AB baked".into(),
            kind: FactoryKind::Bake,
            link: false,
            pack: false,
            keep_members: false,
            mode: FactoryMode::Live,
            built: None,
        };
        let after = place_output(&canon, &f, &["A".into(), "B".into()]);
        let names: Vec<(&str, bool)> = after
            .entries
            .iter()
            .map(|e| (e.name.as_str(), e.enabled))
            .collect();
        assert_eq!(
            names,
            [("A", false), ("B", false), ("AB baked", true), ("C", true)]
        );
        // placing again does not duplicate the output
        assert_eq!(
            place_output(&after, &f, &["A".into(), "B".into()]).entries.len(),
            4
        );
        let one = fingerprint(&f, &[("A".into(), Some(1)), ("B".into(), Some(2))]);
        assert_ne!(
            one,
            fingerprint(&f, &[("A".into(), Some(1)), ("B".into(), Some(3))])
        );
        assert_ne!(
            one,
            fingerprint(
                &Factory {
                    link: true,
                    ..f.clone()
                },
                &[("A".into(), Some(1)), ("B".into(), Some(2))]
            )
        );
    }
}
