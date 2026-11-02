//! Names of chains: a title from what the chain is about, unique names, and names the user gave. @sat:naming
//!
//! Owns: the score map of a chain's features (the monolith's `collectConflictGroupScoreMap` weights, v459), the
//! title picked from it, the suffix that tells equal names apart, and the user's own identities of chains with
//! the rule that carries them over when chains are recomputed (the monolith's context inheritance, Jaccard ≥ 0.28).
//! Not here: emoji and hue from content (`@sat:identity`), the chains (`@sat:overwrite.projection`, `@sat:routes`),
//! where heads are built (`@sat:app.chains.heads`).
//!
//! Kept by meaning, not by code: the monolith ran a pipeline of "mutation" operators over `A → B → …` with a
//! strength and a depth setting; what reached the user was a title of the strongest path and separator words
//! before the route. Here the strength picks how many words the title keeps; the operators are not ported.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Words that say nothing about a chain (the monolith's stop list).
const STOP: &[&str] = &[
    "patch", "mod", "mods", "the", "and", "for", "of", "to", "a", "an", "with", "from", "in", "on", "by",
    "plugin", "plugins", "edition", "version", "fixed", "fix", "updated", "update",
];

/// The monolith's default strength (0..10).
pub const STRENGTH_DEFAULT: u8 = 5;

/// What a chain is made of, for its name.
#[derive(Debug, Clone, Copy)]
pub struct ChainFeatures<'a> {
    /// Mods of the route, winner first.
    pub mods: &'a [&'a str],
    /// Separators of the route's mods (one per mod that has one; repeats count).
    pub separators: &'a [&'a str],
    /// Sample paths (the first ones of the chain).
    pub paths: &'a [String],
    /// Files of the chain.
    pub files: u32,
}

/// Where a scored feature comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Mod,
    Separator,
    Folder,
    Extension,
    Word,
    Shape,
}

fn words(s: &str) -> impl Iterator<Item = String> + '_ {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 3 && !w.chars().all(|c| c.is_ascii_digit()))
        .map(str::to_lowercase)
        .filter(|w| !STOP.contains(&w.as_str()))
}

// @sat:naming.score
/// Scored features of a chain, strongest first (ties by key).
pub fn score_map(f: &ChainFeatures<'_>) -> Vec<(String, u32, Kind)> {
    let mut m: HashMap<String, (u32, Kind)> = HashMap::new();
    let mut add = |k: String, s: u32, kind: Kind| {
        if k.chars().count() >= 2 {
            let e = m.entry(k).or_insert((0, kind));
            e.0 += s;
        }
    };
    let last = f.mods.len().saturating_sub(1);
    for (i, md) in f.mods.iter().enumerate() {
        add(
            md.to_lowercase(),
            4 + if i == 0 || i == last { 3 } else { 1 },
            Kind::Mod,
        );
    }
    for sep in f.separators {
        add(sep.trim().to_lowercase(), 5, Kind::Separator);
        for w in words(sep) {
            add(w, 3, Kind::Word);
        }
    }
    for p in f.paths {
        let p = p.to_lowercase();
        let (dir, file) = p.rsplit_once('/').unwrap_or(("", p.as_str()));
        let folders: Vec<&str> = dir.split('/').filter(|s| !s.is_empty()).collect();
        if let Some(top) = folders.first() {
            add((*top).to_string(), 8, Kind::Folder);
        }
        if let Some(second) = folders.get(1) {
            add((*second).to_string(), 5, Kind::Folder);
        }
        for (idx, folder) in folders.iter().enumerate() {
            for w in words(folder) {
                add(w, 4u32.saturating_sub(idx as u32).max(1), Kind::Word);
            }
        }
        let (stem, ext) = file.rsplit_once('.').unwrap_or((file, ""));
        if !ext.is_empty() {
            add(ext.to_string(), 7, Kind::Extension);
        }
        for w in words(stem) {
            add(w, 3, Kind::Word);
        }
    }
    if f.files >= 10 {
        add("multi file overwrite".into(), 1, Kind::Shape);
    }
    if f.mods.len() >= 4 {
        add("long chain".into(), 1, Kind::Shape);
    }
    let mut v: Vec<(String, u32, Kind)> = m.into_iter().map(|(k, (s, kind))| (k, s, kind)).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v
}

// @sat:naming.title
/// The title of a chain: its strongest folder, separator, extension and word features — as many as the strength
/// gives (default 5 → two), a word already inside a picked one left out (`armor` next to `armor/iron`).
pub fn title(f: &ChainFeatures<'_>, strength: u8) -> String {
    let want = 1 + usize::from(strength.min(10)) / 3;
    let mut picked: Vec<String> = Vec::new();
    for (k, _, kind) in score_map(f) {
        if picked.len() >= want {
            break;
        }
        if matches!(kind, Kind::Mod | Kind::Shape) {
            continue;
        }
        if picked.iter().any(|p| p.contains(&k) || k.contains(p.as_str())) {
            continue;
        }
        picked.push(k);
    }
    picked.join(" · ")
}

/// Short tag of a signature that tells equal names apart (`· 7K2Q`).
pub fn short_tag(signature: u64) -> String {
    const DIGITS: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let mut x = signature;
    let mut out = Vec::new();
    for _ in 0..4 {
        out.push(DIGITS[(x % 36) as usize]);
        x /= 36;
    }
    String::from_utf8(out).expect("ascii")
}

// @sat:naming.unique
/// Gives every name that occurs more than once the short tag of its signature (`name · 7K2Q`).
pub fn make_unique(names: &mut [String], signatures: &[u64]) {
    let mut count: HashMap<&str, usize> = HashMap::new();
    for n in names.iter() {
        *count.entry(n.as_str()).or_default() += 1;
    }
    let dup: Vec<bool> = names.iter().map(|n| count[n.as_str()] > 1).collect();
    for (i, n) in names.iter_mut().enumerate() {
        if dup[i] {
            n.push_str(" · ");
            n.push_str(&short_tag(signatures[i]));
        }
    }
}

mod hex_list {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &[u64], s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(v.iter().map(|x| format!("{x:016x}")))
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u64>, D::Error> {
        let v: Vec<String> = Vec::deserialize(d)?;
        v.iter()
            .map(|s| u64::from_str_radix(s, 16).map_err(serde::de::Error::custom))
            .collect()
    }
}

// @sat:naming.manual
/// A chain identity the user set (name, icon, hue — each optional), with the exact chains it was given to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManualIdentity {
    /// Content signatures of the chain's exact chains when it was named.
    #[serde(with = "hex_list")]
    pub sources: Vec<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emoji: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hue: Option<u16>,
}

impl ManualIdentity {
    pub fn is_empty(&self) -> bool {
        self.name.is_none() && self.emoji.is_none() && self.hue.is_none()
    }
}

/// Least overlap (Jaccard of exact chains) for a chain to keep a name given to an earlier version of it.
pub const INHERIT_AT: f64 = 0.28;

fn jaccard(a: &[u64], b: &[u64]) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 0.0;
    }
    let sa: std::collections::HashSet<&u64> = a.iter().collect();
    let common = b.iter().filter(|x| sa.contains(x)).count();
    common as f64 / (a.len() + b.len() - common) as f64
}

// @sat:naming.inherit
/// Which user identity each chain takes (by index into `manual`): the best overlap of exact chains at least
/// [`INHERIT_AT`], each identity to one chain at most, best pairs first. A chain that split keeps the name in its
/// larger part; two that merged keep the better one.
pub fn inherit(chains: &[&[u64]], manual: &[ManualIdentity]) -> Vec<Option<usize>> {
    let mut pairs: Vec<(f64, usize, usize)> = Vec::new();
    for (c, src) in chains.iter().enumerate() {
        for (m, id) in manual.iter().enumerate() {
            let j = jaccard(src, &id.sources);
            if j >= INHERIT_AT {
                pairs.push((j, c, m));
            }
        }
    }
    pairs.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    let mut out = vec![None; chains.len()];
    let mut taken = vec![false; manual.len()];
    for (_, c, m) in pairs {
        if out[c].is_none() && !taken[m] {
            out[c] = Some(m);
            taken[m] = true;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_come_from_paths_and_separators() {
        let paths: Vec<String> = [
            "textures/armor/iron/cuirass.dds",
            "textures/armor/iron/helmet.dds",
            "textures/armor/steel/boots.dds",
        ]
        .map(String::from)
        .to_vec();
        let f = ChainFeatures {
            mods: &["Iron Retex", "Armor Pack"],
            separators: &["Armor"],
            paths: &paths,
            files: 3,
        };
        let map = score_map(&f);
        assert_eq!(map[0].0, "textures");
        assert_eq!(title(&f, STRENGTH_DEFAULT), "textures · armor");
        assert_eq!(title(&f, 0), "textures");
        assert!(title(&f, 10).starts_with("textures · armor · dds"));
        let lone = ChainFeatures {
            mods: &["A", "B"],
            separators: &[],
            paths: &[],
            files: 1,
        };
        assert_eq!(title(&lone, 5), "", "nothing to say: no title");
    }

    #[test]
    fn equal_names_get_tags() {
        let mut names = vec!["x".to_string(), "y".into(), "x".into()];
        make_unique(&mut names, &[1, 2, 3]);
        assert_eq!(names[1], "y");
        assert!(names[0].starts_with("x · ") && names[2].starts_with("x · ") && names[0] != names[2]);
    }

    #[test]
    fn user_names_follow_their_chains() {
        let id = |src: &[u64], n: &str| ManualIdentity {
            sources: src.to_vec(),
            name: Some(n.into()),
            emoji: None,
            hue: None,
        };
        let manual = [id(&[1, 2, 3, 4], "big"), id(&[9], "nine")];
        // the chain grew by one exact chain, another lost its only one
        let chains: [&[u64]; 3] = [&[7], &[1, 2, 3, 4, 5], &[1]];
        assert_eq!(inherit(&chains, &manual), [None, Some(0), None]);
        // a split: the larger part keeps the name
        let chains: [&[u64]; 2] = [&[1], &[2, 3, 4]];
        assert_eq!(inherit(&chains, &manual), [None, Some(0)]);
        let json = serde_json::to_string(&manual[1]).unwrap();
        assert_eq!(json, r#"{"sources":["0000000000000009"],"name":"nine"}"#);
        assert_eq!(serde_json::from_str::<ManualIdentity>(&json).unwrap(), manual[1]);
    }
}
