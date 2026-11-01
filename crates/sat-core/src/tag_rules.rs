//! Tag rule sets: tags a mod gets from what is in it (ADR 0013 §3). @sat:tag-rules
//!
//! Owns: the rule set format (the monolith's `BUILTIN_TAG_DETECTORS` as a data file), compiling a set once, and
//! the tags of one mod from its facts with scores.
//! Not here: tags the user writes (`@sat:order.decor.marks`), colours (`@sat:tags`), where the sets come from
//! (built-in file `rules/tags/monolith-builtin.json`; community sets through resources, ADR 0011).
//!
//! Matching is the monolith's (`classifyTagsFromDeepContext`, area 17), case-folded:
//! - `path`, `file`, `plugin`, `name` — the needle is a substring of a loose path / a file name / a plugin name /
//!   the mod name; `folder` — a folder of that name anywhere; `root` — a top-level folder; `ext` — an extension;
//! - a group matches when any of its positive fields matches (or it has none), every group in `all` matches and no
//!   group in `none` does;
//! - every matching rule adds its `score` to its tag; tags at or above the set's threshold are kept, best first.
//!
//! Speed: the monolith searched each needle in a joined text of all paths — rules × needles × paths. Here every
//! needle of a field goes into one Aho-Corasick automaton built once per set; a mod's paths are read once.

use aho_corasick::AhoCorasick;
use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};

use crate::facts::ModFacts;

/// One group of conditions (a rule is a group with a tag and a score).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Group {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub file: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plugin: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub name: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub folder: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub root: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ext: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub all: Vec<Group>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub none: Vec<Group>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    pub tag: String,
    #[serde(default = "five")]
    pub score: u32,
    #[serde(flatten)]
    pub when: Group,
}

fn five() -> u32 {
    5
}

// @sat:tag-rules.set
/// A rule set file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleSet {
    pub format: String,
    pub version: u32,
    pub name: String,
    #[serde(default)]
    pub source: String,
    #[serde(default = "five")]
    pub threshold: u32,
    /// How `path`, `file`, `plugin`, `name` needles match. Version 1 (the monolith's format): a substring —
    /// `"ui"` also hits `build` and `quiet`. Version 2 defaults to whole words (letters and digits around the
    /// needle are not part of it); a set may say either explicitly.
    #[serde(default, rename = "match", skip_serializing_if = "Option::is_none")]
    pub matching: Option<Matching>,
    pub rules: Vec<Rule>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Matching {
    Substring,
    Word,
}

impl RuleSet {
    pub fn matching(&self) -> Matching {
        self.matching.unwrap_or(if self.version >= 2 {
            Matching::Word
        } else {
            Matching::Substring
        })
    }
}

/// A tag found on a mod, and why: the rules (indexes into [`RuleSet::rules`]) that gave it its score.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FoundTag {
    pub tag: String,
    pub score: u32,
    pub rules: Vec<u32>,
}

/// The built-in set (the monolith's detectors).
pub const BUILTIN: &str = include_str!("../../../rules/tags/monolith-builtin.json");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Field {
    Path,
    File,
    Plugin,
    Name,
}

/// A rule set ready to run.
#[derive(Debug)]
pub struct Compiled {
    set: RuleSet,
    /// One automaton per text field; pattern id → needle index in `needles`.
    automata: Vec<(Field, AhoCorasick, Vec<u32>)>,
    needle_ids: FxHashMap<(Field, String), u32>,
}

fn norm(s: &str) -> String {
    s.trim().replace('\\', "/").to_lowercase()
}

/// What one mod offers to the matcher.
#[derive(Debug, Default)]
struct Seen {
    hits: FxHashSet<u32>,
    folders: FxHashSet<String>,
    roots: FxHashSet<String>,
    exts: FxHashSet<String>,
}

fn ext_of(name: &str) -> Option<String> {
    name.rfind('.').map(|i| name[i..].to_string())
}

impl Compiled {
    pub fn new(set: RuleSet) -> Result<Self, String> {
        let mut per: FxHashMap<Field, Vec<String>> = FxHashMap::default();
        let mut needle_ids: FxHashMap<(Field, String), u32> = FxHashMap::default();
        fn collect(
            g: &Group,
            per: &mut FxHashMap<Field, Vec<String>>,
            ids: &mut FxHashMap<(Field, String), u32>,
        ) {
            for (f, v) in [
                (Field::Path, &g.path),
                (Field::File, &g.file),
                (Field::Plugin, &g.plugin),
                (Field::Name, &g.name),
            ] {
                for n in v.iter().map(|n| norm(n)).filter(|n| !n.is_empty()) {
                    let next = ids.len() as u32;
                    if let std::collections::hash_map::Entry::Vacant(e) = ids.entry((f, n.clone())) {
                        e.insert(next);
                        per.entry(f).or_default().push(n);
                    }
                }
            }
            for s in g.all.iter().chain(&g.none) {
                collect(s, per, ids);
            }
        }
        for r in &set.rules {
            collect(&r.when, &mut per, &mut needle_ids);
        }
        let mut automata = Vec::new();
        for (f, needles) in per {
            let ac = AhoCorasick::new(&needles).map_err(|e| e.to_string())?;
            let ids = needles.iter().map(|n| needle_ids[&(f, n.clone())]).collect();
            automata.push((f, ac, ids));
        }
        Ok(Self {
            set,
            automata,
            needle_ids,
        })
    }

    pub fn builtin() -> Self {
        let set: RuleSet = serde_json::from_str(BUILTIN).expect("the built-in rule set parses");
        Self::new(set).expect("the built-in rule set compiles")
    }

    pub fn set(&self) -> &RuleSet {
        &self.set
    }

    fn seen(&self, m: &ModFacts) -> Seen {
        let mut s = Seen::default();
        let word = self.set.matching() == Matching::Word;
        let alnum_at = |text: &str, i: usize, before: bool| {
            let c = if before {
                text[..i].chars().next_back()
            } else {
                text[i..].chars().next()
            };
            c.is_some_and(char::is_alphanumeric)
        };
        let mut scan = |field: Field, text: &str| {
            for (f, ac, ids) in &self.automata {
                if *f == field {
                    for hit in ac.find_overlapping_iter(text) {
                        if word && (alnum_at(text, hit.start(), true) || alnum_at(text, hit.end(), false)) {
                            continue;
                        }
                        s.hits.insert(ids[hit.pattern().as_usize()]);
                    }
                }
            }
        };
        let name = norm(&m.name);
        scan(Field::Name, &name);
        for p in &m.plugins {
            let n = norm(&p.file_name);
            scan(Field::Plugin, &n);
            scan(Field::Path, &n); // the monolith counts plugin names as paths too
            if let Some(e) = ext_of(&n) {
                s.exts.insert(e);
            }
        }
        for d in &m.dirs {
            let rel = norm(&d.rel);
            let parts: Vec<&str> = rel.split('/').filter(|p| !p.is_empty()).collect();
            if let Some(r) = parts.first() {
                s.roots.insert((*r).to_string());
            }
            for p in &parts {
                s.folders.insert((*p).to_string());
            }
            for f in &d.files {
                let file = norm(&f.name);
                let path = if rel.is_empty() {
                    file.clone()
                } else {
                    format!("{rel}/{file}")
                };
                scan(Field::Path, &path);
                scan(Field::File, &file);
                if let Some(e) = ext_of(&file) {
                    s.exts.insert(e);
                }
            }
        }
        s
    }

    fn group(&self, g: &Group, s: &Seen) -> bool {
        let hit = |f: Field, v: &[String]| {
            v.iter().any(|n| {
                self.needle_ids
                    .get(&(f, norm(n)))
                    .is_some_and(|id| s.hits.contains(id))
            })
        };
        let has_positive = [&g.path, &g.file, &g.plugin, &g.name, &g.folder, &g.root, &g.ext]
            .iter()
            .any(|v| !v.is_empty());
        let positive = !has_positive
            || hit(Field::Path, &g.path)
            || hit(Field::File, &g.file)
            || hit(Field::Plugin, &g.plugin)
            || hit(Field::Name, &g.name)
            || g.folder.iter().any(|f| s.folders.contains(&norm(f)))
            || g.root.iter().any(|r| s.roots.contains(&norm(r)))
            || g.ext.iter().any(|e| s.exts.contains(&norm(e)));
        positive && g.all.iter().all(|x| self.group(x, s)) && !g.none.iter().any(|x| self.group(x, s))
    }

    // @sat:tag-rules.apply
    /// Tags of one mod with their scores and the rules behind them, best first (at or above the threshold).
    /// "Why does this mod have that tag" always has an answer — a set from the community must be debuggable.
    pub fn found(&self, m: &ModFacts) -> Vec<FoundTag> {
        if m.missing {
            return Vec::new();
        }
        let s = self.seen(m);
        let mut by_tag: FxHashMap<&str, FoundTag> = FxHashMap::default();
        for (i, r) in self.set.rules.iter().enumerate() {
            if self.group(&r.when, &s) {
                let t = by_tag.entry(r.tag.as_str()).or_insert_with(|| FoundTag {
                    tag: r.tag.clone(),
                    score: 0,
                    rules: Vec::new(),
                });
                t.score += r.score;
                t.rules.push(i as u32);
            }
        }
        let mut out: Vec<FoundTag> = by_tag
            .into_values()
            .filter(|t| t.score >= self.set.threshold)
            .collect();
        out.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.tag.cmp(&b.tag)));
        out
    }

    /// Tags of one mod with their scores, best first.
    pub fn tags(&self, m: &ModFacts) -> Vec<(String, u32)> {
        self.found(m).into_iter().map(|t| (t.tag, t.score)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::{LooseDir, LooseFile, PluginFacts};

    fn facts(name: &str, paths: &[&str], plugins: &[&str]) -> ModFacts {
        let mut dirs: Vec<LooseDir> = Vec::new();
        for p in paths {
            let (dir, file) = p.rsplit_once('/').unwrap_or(("", p));
            let f = LooseFile {
                name: file.into(),
                size: 1,
                vram: 0,
            };
            match dirs.iter_mut().find(|d| d.rel == dir) {
                Some(d) => d.files.push(f),
                None => dirs.push(LooseDir {
                    rel: dir.into(),
                    files: vec![f],
                }),
            }
        }
        ModFacts {
            name: name.into(),
            dirs,
            plugins: plugins
                .iter()
                .map(|p| PluginFacts {
                    file_name: p.to_string(),
                    header: None,
                    records: None,
                    error: None,
                })
                .collect(),
            ..ModFacts::default()
        }
    }

    #[test]
    fn the_builtin_set_loads_with_every_monolith_detector() {
        let c = Compiled::builtin();
        assert_eq!(c.set().rules.len(), 257);
        assert_eq!(c.set().threshold, 5);
    }

    #[test]
    fn skse_plugins_with_and_without_plugins() {
        let c = Compiled::builtin();
        let pure = c.tags(&facts(
            "Some Fix",
            &["SKSE/Plugins/fix.dll", "SKSE/Plugins/fix.ini"],
            &[],
        ));
        let names: Vec<&str> = pure.iter().map(|(t, _)| t.as_str()).collect();
        assert!(
            names.contains(&"SKSE Plugin") && names.contains(&"pure-skse-plugin"),
            "{names:?}"
        );
        // the same with a plugin file: `none` blocks the pure tag
        let mixed = c.tags(&facts("Some Fix", &["SKSE/Plugins/fix.dll"], &["Fix.esp"]));
        let names: Vec<&str> = mixed.iter().map(|(t, _)| t.as_str()).collect();
        assert!(
            names.contains(&"SKSE Plugin") && !names.contains(&"pure-skse-plugin"),
            "{names:?}"
        );
        // scores sort the tags, best first
        assert!(pure.windows(2).all(|w| w[0].1 >= w[1].1));
    }

    #[test]
    fn a_custom_set_with_all_none_root_and_ext() {
        let set: RuleSet = serde_json::from_value(serde_json::json!({
            "format": "satellite-tag-rules", "version": 1, "name": "test", "threshold": 5,
            "rules": [
                {"tag": "Scripts", "score": 5, "root": ["scripts"], "ext": [".pex"]},
                {"tag": "Weak", "score": 2, "name": ["mod"]},
                {"tag": "Weak", "score": 2, "file": ["readme"]},
                {"tag": "Textures only", "score": 6, "all": [{"root": ["textures"]}], "none": [{"ext": [".esp", ".nif"]}]}
            ]
        }))
        .unwrap();
        let c = Compiled::new(set).unwrap();
        let t = |f: &ModFacts| c.tags(f).into_iter().map(|(t, _)| t).collect::<Vec<_>>();
        assert_eq!(
            t(&facts("My Mod", &["scripts/a.pex", "readme.txt"], &[])),
            ["Scripts"],
            "2 + 2 < 5"
        );
        assert_eq!(t(&facts("Tex", &["textures/a.dds"], &[])), ["Textures only"]);
        assert!(t(&facts("Tex", &["textures/a.dds", "meshes/a.nif"], &[])).is_empty());
        assert!(c.tags(&ModFacts::missing("Gone")).is_empty());
    }

    #[test]
    fn version_2_matches_whole_words_and_every_tag_says_why() {
        let rules = serde_json::json!([
            {"tag": "UI", "score": 5, "name": ["ui"]},
            {"tag": "UI", "score": 3, "path": ["interface"]}
        ]);
        let set = |version: u32| -> RuleSet {
            serde_json::from_value(serde_json::json!({
                "format": "satellite-tag-rules", "version": version, "name": "t", "rules": rules
            }))
            .unwrap()
        };
        let quiet = facts("Quiet Build", &["sound/a.wav"], &[]);
        let hud = facts("Better UI-HUD", &["interface/hud.swf"], &[]);
        let v1 = Compiled::new(set(1)).unwrap();
        assert_eq!(
            v1.tags(&quiet).len(),
            1,
            "version 1 (the monolith): `ui` inside `quiet` counts"
        );
        let v2 = Compiled::new(set(2)).unwrap();
        assert!(v2.tags(&quiet).is_empty(), "version 2: whole words only");
        let found = v2.found(&hud);
        assert_eq!(
            (found[0].tag.as_str(), found[0].score, found[0].rules.as_slice()),
            ("UI", 8, &[0u32, 1][..])
        );
    }
}
