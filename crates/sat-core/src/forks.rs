//! Forks of taste: one aspect of the build, its options, the choice in force and who made it. @sat:forks
//!
//! Owns: a fork of the project — the aspect (a head, a lighting base, a version of one mod), what it is about, the
//! options (each a set of mods with the facts that tell them apart), the option in force, who chose it (Claude by
//! default, the owner's word is final) and why; what choosing an option asks of the mod list (its mods on, the mods
//! only the other options have off).
//! Not here: changing the list (`@sat:order.book`, the CLI `@sat:cli.forks`), finding the candidates (usurper tags,
//! the what-if table `@sat:what-if`).
//!
//! The owner settles taste; Claude prepares the fork, applies a reasoned default and leaves the final word to him.
//! Mods are kept by name, as the canon names them.

use serde::{Deserialize, Serialize};

/// Who chose the option in force.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ChosenBy {
    /// Not chosen yet: the list stays as it is.
    #[default]
    Nobody,
    /// Claude's default, with a reason; the owner may override it.
    Claude,
    /// The owner's word: final.
    Owner,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForkOption {
    pub name: String,
    /// The mods this option turns on (canon names).
    pub mods: Vec<String>,
    /// What tells it apart: version, game edition, size, what it wins, what it needs.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub facts: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fork {
    /// Stable id (`f-N`).
    pub id: String,
    /// The aspect: "голова", "SkyParkour: версия".
    pub aspect: String,
    /// Why it is a fork and what to look at.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub about: String,
    pub options: Vec<ForkOption>,
    /// The option in force (index into `options`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chosen: Option<usize>,
    #[serde(default)]
    pub by: ChosenBy,
    /// Why this option.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub why: String,
}

/// What choosing an option asks of the mod list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Effect {
    /// The chosen option's mods.
    pub enable: Vec<String>,
    /// Mods only the other options have.
    pub disable: Vec<String>,
}

impl Fork {
    // @sat:forks.effect
    /// The mods to turn on and off for option `i` (`None` — no such option).
    pub fn effect(&self, i: usize) -> Option<Effect> {
        let chosen = self.options.get(i)?;
        let keep = |m: &String| chosen.mods.iter().any(|c| c.eq_ignore_ascii_case(m));
        let mut disable: Vec<String> = Vec::new();
        for (j, o) in self.options.iter().enumerate() {
            if j == i {
                continue;
            }
            for m in &o.mods {
                if !keep(m) && !disable.iter().any(|d| d.eq_ignore_ascii_case(m)) {
                    disable.push(m.clone());
                }
            }
        }
        Some(Effect {
            enable: chosen.mods.clone(),
            disable,
        })
    }

    /// Whether the owner has settled it.
    pub fn settled(&self) -> bool {
        self.by == ChosenBy::Owner
    }
}

// @sat:forks.id
/// A new fork id not used by `forks`.
pub fn fresh_id(forks: &[Fork]) -> String {
    (1..)
        .map(|n| format!("f-{n}"))
        .find(|id| !forks.iter().any(|f| f.id == *id))
        .expect("an unused id")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opt(name: &str, mods: &[&str]) -> ForkOption {
        ForkOption {
            name: name.into(),
            mods: mods.iter().map(|m| m.to_string()).collect(),
            facts: String::new(),
        }
    }

    #[test]
    fn choosing_an_option_turns_its_mods_on_and_only_the_others_off() {
        let f = Fork {
            id: "f-1".into(),
            aspect: "голова".into(),
            about: String::new(),
            options: vec![
                opt("A", &["Head A", "Shared patch"]),
                opt("B", &["Head B", "shared PATCH"]),
                opt("C", &["Head C"]),
            ],
            chosen: None,
            by: ChosenBy::Nobody,
            why: String::new(),
        };
        let e = f.effect(1).unwrap();
        assert_eq!(e.enable, ["Head B", "shared PATCH"]);
        // the patch both A and B have stays on; A's and C's own mods go off
        assert_eq!(e.disable, ["Head A", "Head C"]);
        assert!(f.effect(3).is_none());
        assert_eq!(fresh_id(&[f]), "f-2");
    }
}
