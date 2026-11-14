//! Conflict domains described by data (ADR 0018 §3). @sat:conflict.spec
//!
//! A path pattern with captures says what a participant and a target are, a centrality says where the order comes
//! from.
//!
//! Owns: the spec (`DomainSpec`: pattern, participant and target templates, centrality, semantics, the field that
//! ranks a participant) and its evaluation over the provider table — the files the game sees (each path's
//! highest-priority active copy), grouped into contests, ranked, and folded into a `DomainReport`.
//! Not here: reading files (the caller passes a reader by provider index), the game's specs (`@sat:game`), the
//! shape of a report (`@sat:conflict`).
//!
//! The engine knows no game: OAR is a spec in the Skyrim pack, a user's own centrality is a spec in a user file.
//! A participant whose rank cannot be read is left out and named in the report's notes, never guessed.

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

use crate::conflict::{
    Builder, Centrality, DomainInfo, DomainReport, Family, Origin, Semantics, Severity, cell, ranked_cells,
};
use crate::ids::PathId;
use crate::overwrite::Priorities;
use crate::table::ProviderTable;

/// Where a participant's rank comes from (centrality `field`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FieldSpec {
    /// Files to read, the first found counts (templates over the captures).
    #[serde(default)]
    pub files: Vec<String>,
    /// A JSON pointer (`/priority`) or an INI key (`Section.key`).
    #[serde(default)]
    pub key: Option<String>,
    /// Or: the rank is this capture, read as a number.
    #[serde(default)]
    pub capture: Option<String>,
    /// A participant whose files say true here is out (a disabled submod).
    #[serde(default)]
    pub skip_if: Option<String>,
    /// Rank when the field is absent; without it such a participant is left out.
    #[serde(default)]
    pub default: Option<f64>,
}

fn collision() -> Family {
    Family::Collision
}

fn two() -> usize {
    2
}

// @sat:conflict.spec.shape
/// One domain described by data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DomainSpec {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub about: String,
    #[serde(default = "collision")]
    pub family: Family,
    /// A path with `{name}` captures, each one path segment (case does not matter).
    pub pattern: String,
    /// Only these extensions of the last segment (empty — any).
    #[serde(default)]
    pub ext: Vec<String>,
    /// Who takes part: a template over the captures.
    pub participant: String,
    /// What they compete for: a template over the captures.
    pub target: String,
    pub centrality: Centrality,
    pub semantics: Semantics,
    #[serde(default)]
    pub field: Option<FieldSpec>,
    /// Participants a target needs to be a case.
    #[serde(default = "two")]
    pub min_participants: usize,
    /// Only files whose name (the last segment) matches this regex.
    #[serde(default)]
    pub file: Option<String>,
    /// Targets are lines of the matched files, not the files: rule files (`*_SWAP.ini`, `*_DISTR.ini`).
    #[serde(default)]
    pub lines: Option<LineSpec>,
}

// @sat:conflict.spec.lines
/// A rule file's lines as targets: each line the regex matches is one target, its named groups join the captures
/// (`target` names them). `;` and `#` start comments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LineSpec {
    pub regex: String,
}

#[derive(Debug)]
enum Seg {
    Lit(String),
    Cap(String),
}

fn compile(pattern: &str) -> Vec<Seg> {
    pattern
        .trim_matches('/')
        .split('/')
        .map(|s| match s.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
            Some(c) => Seg::Cap(c.to_string()),
            None => Seg::Lit(s.to_lowercase()),
        })
        .collect()
}

fn matches(segs: &[Seg], path: &str) -> Option<Vec<(String, String)>> {
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() != segs.len() {
        return None;
    }
    let mut caps = Vec::new();
    for (s, p) in segs.iter().zip(&parts) {
        match s {
            Seg::Lit(l) => {
                if !p.eq_ignore_ascii_case(l) {
                    return None;
                }
            }
            Seg::Cap(c) => caps.push((c.clone(), p.to_string())),
        }
    }
    Some(caps)
}

/// `{name}` → its capture.
pub fn render(template: &str, caps: &[(String, String)]) -> String {
    let mut out = template.to_string();
    for (k, v) in caps {
        out = out.replace(&format!("{{{k}}}"), v);
    }
    out
}

/// A value by JSON pointer or INI `Section.key`.
fn lookup(bytes: &[u8], key: &str) -> Option<serde_json::Value> {
    let text = String::from_utf8_lossy(bytes);
    let text = text.trim_start_matches('\u{feff}');
    if key.starts_with('/') {
        let v: serde_json::Value = serde_json::from_str(text).ok()?;
        return v.pointer(key).cloned();
    }
    let (section, name) = key.split_once('.').unwrap_or(("", key));
    let mut cur = String::new();
    for line in text.lines() {
        let l = line.trim();
        if let Some(s) = l.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            cur = s.trim().to_string();
        } else if let Some((k, v)) = l.split_once('=')
            && cur.eq_ignore_ascii_case(section)
            && k.trim().eq_ignore_ascii_case(name)
        {
            return Some(serde_json::Value::String(v.trim().to_string()));
        }
    }
    None
}

fn number(v: &serde_json::Value) -> Option<f64> {
    match v {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.trim().parse().ok(),
        serde_json::Value::Bool(b) => Some(f64::from(u8::from(*b))),
        _ => None,
    }
}

fn truthy(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Bool(b) => *b,
        serde_json::Value::Number(n) => n.as_f64().is_some_and(|x| x != 0.0),
        serde_json::Value::String(s) => matches!(s.trim().to_lowercase().as_str(), "1" | "true" | "yes"),
        _ => false,
    }
}

/// The copy of a path the game sees: of the highest-priority active owner, loose before archive.
fn seen(table: &ProviderTable, prio: &Priorities, p: PathId) -> Option<usize> {
    table
        .path_providers(p)
        .filter(|&i| prio.is_active(table.owner(i)))
        .max_by_key(|&i| (prio.get(table.owner(i)), !table.is_archive(i)))
}

struct Member {
    caps: Vec<(String, String)>,
    owner: usize,
}

/// A compiled pattern: matches a path and gives its captures.
#[derive(Debug)]
pub struct Matcher {
    segs: Vec<Seg>,
    prefix: String,
    exts: Vec<String>,
    file: Option<regex::Regex>,
}

impl Matcher {
    pub fn new(spec: &DomainSpec) -> Self {
        let segs = compile(&spec.pattern);
        let prefix: String = segs
            .iter()
            .take_while(|s| matches!(s, Seg::Lit(_)))
            .map(|s| match s {
                Seg::Lit(l) => format!("{l}/"),
                Seg::Cap(_) => String::new(),
            })
            .collect();
        let exts = spec
            .ext
            .iter()
            .map(|e| format!(".{}", e.to_lowercase()))
            .collect();
        let file = spec.file.as_deref().and_then(|f| regex::Regex::new(f).ok());
        Self {
            segs,
            prefix,
            exts,
            file,
        }
    }

    /// Captures of a path (`lower` — the same path in lower case, to test cheaply), if it matches.
    pub fn captures(&self, path: &str, lower: &str) -> Option<Vec<(String, String)>> {
        if !lower.starts_with(&self.prefix) {
            return None;
        }
        if !self.exts.is_empty() && !self.exts.iter().any(|e| lower.ends_with(e.as_str())) {
            return None;
        }
        if let Some(f) = &self.file
            && !f.is_match(path.rsplit('/').next().unwrap_or(path))
        {
            return None;
        }
        matches(&self.segs, path)
    }
}

/// Paths a spec matches, with their captures (they depend on the table only, not on the order).
pub type Matched = Vec<(PathId, Vec<(String, String)>)>;

/// Matches of a spec over the whole table.
pub fn matched(spec: &DomainSpec, table: &ProviderTable) -> Matched {
    let m = Matcher::new(spec);
    (0..table.path_count())
        .filter_map(|p| {
            let p = PathId::from_idx(p);
            let s = table.path_string(p);
            m.captures(&s, &s.to_lowercase()).map(|c| (p, c))
        })
        .collect()
}

// @sat:conflict.spec.evaluate
/// Evaluates a spec over the table. `read(provider)` gives a file's bytes (for `field` ranks).
pub fn evaluate(
    spec: &DomainSpec,
    origin: Origin,
    table: &ProviderTable,
    prio: &Priorities,
    read: &dyn Fn(usize) -> Option<Vec<u8>>,
) -> DomainReport {
    evaluate_matched(spec, origin, table, prio, read, &matched(spec, table))
}

/// [`evaluate`] over paths already matched.
pub fn evaluate_matched(
    spec: &DomainSpec,
    origin: Origin,
    table: &ProviderTable,
    prio: &Priorities,
    read: &dyn Fn(usize) -> Option<Vec<u8>>,
    paths: &Matched,
) -> DomainReport {
    let info = DomainInfo {
        id: spec.id.clone(),
        title: spec.title.clone(),
        about: spec.about.clone(),
        family: spec.family,
        centrality: spec.centrality,
        semantics: spec.semantics,
        origin,
    };
    // target → participant → member
    let mut targets: FxHashMap<String, FxHashMap<String, Member>> = FxHashMap::default();
    let line_re = spec.lines.as_ref().and_then(|l| regex::Regex::new(&l.regex).ok());
    let mut unread_files = 0u32;
    for (p, caps) in paths {
        let Some(w) = seen(table, prio, *p) else { continue };
        let who = render(&spec.participant, caps);
        let mut add = |caps: Vec<(String, String)>| {
            let what = render(&spec.target, &caps).to_lowercase();
            targets
                .entry(what)
                .or_default()
                .entry(who.to_lowercase())
                .or_insert(Member {
                    caps,
                    owner: table.owner(w).idx(),
                });
        };
        match &line_re {
            None => add(caps.clone()),
            Some(re) => {
                let Some(bytes) = read(w) else {
                    unread_files += 1;
                    continue;
                };
                for line in String::from_utf8_lossy(&bytes).lines() {
                    let l = line.split([';', '#']).next().unwrap_or("");
                    let Some(m) = re.captures(l) else { continue };
                    let mut c = caps.clone();
                    for name in re.capture_names().flatten() {
                        if let Some(v) = m.name(name) {
                            c.push((name.to_string(), v.as_str().trim().to_string()));
                        }
                    }
                    add(c);
                }
            }
        }
    }

    let mut b = Builder::new(info);
    let mut rank_cache: FxHashMap<String, Option<f64>> = FxHashMap::default();
    let mut skipped: Vec<String> = Vec::new();
    let mut keys: Vec<&String> = targets.keys().collect();
    keys.sort();
    for t in keys {
        let members = &targets[t];
        if members.len() < spec.min_participants {
            continue;
        }
        // rank every member
        let mut ranked: Vec<(String, f64, usize)> = Vec::new();
        for (key, m) in members {
            let label = render(&spec.participant, &m.caps);
            let rank = match spec.centrality {
                Centrality::ModOrder | Centrality::PluginOrder => {
                    Some(f64::from(prio.get(crate::ids::ModId::from_idx(m.owner))))
                }
                Centrality::Name => Some(0.0),
                Centrality::None => Some(0.0),
                Centrality::Field => {
                    let f = spec.field.clone().unwrap_or_default();
                    match &f.capture {
                        Some(c) => m
                            .caps
                            .iter()
                            .find(|x| &x.0 == c)
                            .and_then(|x| x.1.trim().parse::<f64>().ok())
                            .or(f.default),
                        None => *rank_cache
                            .entry(key.clone())
                            .or_insert_with(|| field_rank(&f, &m.caps, table, prio, read)),
                    }
                }
            };
            match rank {
                Some(r) if r.is_finite() => ranked.push((label, r, m.owner)),
                Some(_) => {}
                None => skipped.push(label),
            }
        }
        if ranked.len() < spec.min_participants {
            continue;
        }
        if spec.centrality == Centrality::Name {
            ranked.sort_by_key(|a| a.0.to_lowercase());
            for (i, r) in ranked.iter_mut().enumerate() {
                r.1 = i as f64;
            }
        }
        let higher = spec.semantics != Semantics::LowestWins;
        // stack: lowest first, the winner last
        ranked.sort_by(|a, b| {
            let o = a.1.total_cmp(&b.1);
            (if higher { o } else { o.reverse() }).then(a.0.cmp(&b.0))
        });
        let ids: Vec<u32> = ranked
            .iter()
            .map(|(label, r, owner)| {
                b.participant(
                    label,
                    Some(table.mod_name(crate::ids::ModId::from_idx(*owner))),
                    *r as i64,
                )
            })
            .collect();
        let (cells, tie) = if spec.semantics.has_winner() {
            let ranks: Vec<f64> = ranked.iter().map(|x| x.1).collect();
            ranked_cells(&ranks, higher)
        } else {
            (vec![cell::MEMBER; ranked.len()], false)
        };
        let members: Vec<(u32, u8)> = ids.iter().copied().zip(cells).collect();
        let (kind, sev, unresolved) = if tie {
            ("ничья", Severity::Warn, true)
        } else if spec.semantics == Semantics::Conditional {
            ("по условиям", Severity::Info, false)
        } else if spec.semantics == Semantics::Exclusive {
            ("взаимоисключение", Severity::Error, true)
        } else {
            let how = match spec.centrality {
                Centrality::Name => "по имени файла",
                Centrality::Field => "по приоритету",
                Centrality::ModOrder | Centrality::PluginOrder => "по порядку",
                Centrality::None => "вместе",
            };
            (how, Severity::Info, false)
        };
        b.contest(t, &members, kind, sev, unresolved);
    }
    if unread_files > 0 {
        b.note(format!(
            "{unread_files} файл(ов) правил не прочитано (в архиве или нет доступа)"
        ));
    }
    skipped.sort();
    skipped.dedup();
    if !skipped.is_empty() {
        b.note(format!(
            "без ранга (поле не прочитано): {}{}",
            skipped.iter().take(8).cloned().collect::<Vec<_>>().join(", "),
            if skipped.len() > 8 {
                format!(" и ещё {}", skipped.len() - 8)
            } else {
                String::new()
            }
        ));
    }
    b.finish()
}

/// A participant's rank from its files: the first file found that has the key; `None` when out (`skip_if`) or
/// unread without a default.
fn field_rank(
    f: &FieldSpec,
    caps: &[(String, String)],
    table: &ProviderTable,
    prio: &Priorities,
    read: &dyn Fn(usize) -> Option<Vec<u8>>,
) -> Option<f64> {
    for file in &f.files {
        let path = render(file, caps);
        let Some(p) = table.find_path(&path) else { continue };
        let Some(i) = seen(table, prio, p) else { continue };
        let Some(bytes) = read(i) else { continue };
        if let Some(s) = &f.skip_if
            && lookup(&bytes, s).is_some_and(|v| truthy(&v))
        {
            return Some(f64::NAN);
        }
        if let Some(v) = f
            .key
            .as_deref()
            .and_then(|k| lookup(&bytes, k))
            .and_then(|v| number(&v))
        {
            return Some(v);
        }
    }
    f.default
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::tests::mk;

    fn oar() -> DomainSpec {
        crate::game::builtin()[0]
            .domains
            .iter()
            .find(|d| d.id == "oar")
            .unwrap()
            .clone()
    }

    #[test]
    fn oar_submods_are_ranked_by_their_own_priority_not_by_the_mod_order() {
        let base = "meshes/actors/character/animations/OpenAnimationReplacer";
        let facts = [
            mk(
                "Low",
                &[
                    &format!("{base}/Pack A/Heavy/mt_idle.hkx"),
                    &format!("{base}/Pack A/Heavy/config.json"),
                ],
            ),
            mk(
                "High",
                &[
                    &format!("{base}/Pack B/Light/mt_idle.hkx"),
                    &format!("{base}/Pack B/Light/config.json"),
                    &format!("{base}/Pack B/Same/mt_idle.hkx"),
                    &format!("{base}/Pack B/Same/config.json"),
                    &format!("{base}/Pack B/Off/mt_idle.hkx"),
                    &format!("{base}/Pack B/Off/config.json"),
                    &format!("{base}/Pack B/Light/mt_walk.hkx"),
                ],
            ),
        ];
        let table = ProviderTable::build(&facts);
        let prio = Priorities(vec![0, 1]);
        let read = |i: usize| -> Option<Vec<u8>> {
            let p = table.path_string(table.path_of_provider(i)).to_lowercase();
            Some(
                if p.contains("/heavy/") {
                    r#"{"priority": 500}"#
                } else if p.contains("/light/") {
                    r#"{"priority": 100}"#
                } else if p.contains("/same/") {
                    r#"{"priority": 500}"#
                } else {
                    r#"{"priority": 9000, "disabled": true}"#
                }
                .as_bytes()
                .to_vec(),
            )
        };
        let r = evaluate(&oar(), Origin::Pack, &table, &prio, &read);
        // mt_idle: Heavy 500 and Same 500 tie above Light 100; Off is disabled; mt_walk has one submod
        assert_eq!(r.counts.targets, 1);
        assert_eq!(r.counts.unresolved, 1);
        let c = &r.chains[0];
        assert_eq!(c.kind, "ничья");
        let labels: Vec<&str> = c
            .stack
            .iter()
            .map(|&p| r.participants[p as usize].label.as_str())
            .collect();
        assert_eq!(labels, ["Pack B/Light", "Pack A/Heavy", "Pack B/Same"]);
        assert_eq!(c.row(0), b"l");
        assert_eq!(c.row(1), b"t");
        assert_eq!(r.participants[c.stack[1] as usize].owner.as_deref(), Some("Low"));
        assert_eq!(r.targets[c.targets[0] as usize], "character/mt_idle.hkx");
    }

    #[test]
    fn rule_files_compete_line_by_line_in_name_order() {
        let facts = [
            mk("A", &["Zeta_SWAP.ini", "Readme.txt"]),
            mk("B", &["Alpha_SWAP.ini"]),
        ];
        let table = ProviderTable::build(&facts);
        let bos = crate::game::builtin()[0]
            .domains
            .iter()
            .find(|d| d.id == "bos")
            .unwrap()
            .clone();
        let read = |i: usize| -> Option<Vec<u8>> {
            let p = table.path_string(table.path_of_provider(i)).to_lowercase();
            Some(if p.starts_with("zeta") {
                b"[Forms]\n0x123~Skyrim.esm|0x456~Skyrim.esm\n; 0x999|x\n".to_vec()
            } else {
                b"[Forms]\n0x123~Skyrim.esm|0x777~Skyrim.esm\nOnlyMine|x\n".to_vec()
            })
        };
        let r = evaluate(&bos, Origin::Pack, &table, &Priorities(vec![0, 1]), &read);
        assert_eq!(
            r.counts.targets, 1,
            "one object swapped by both; comments and single swaps are not cases"
        );
        let c = &r.chains[0];
        let labels: Vec<&str> = c
            .stack
            .iter()
            .map(|&p| r.participants[p as usize].label.as_str())
            .collect();
        assert_eq!(
            labels,
            ["Alpha_SWAP.ini", "Zeta_SWAP.ini"],
            "alphabetical: the last file wins"
        );
        assert_eq!(c.row(1), b"w");
        assert_eq!(r.targets[c.targets[0] as usize], "0x123~skyrim.esm");
    }

    #[test]
    fn a_capture_or_an_ini_key_ranks_too() {
        let base = "meshes/actors/character/animations/DynamicAnimationReplacer/_CustomConditions";
        let facts = [mk(
            "A",
            &[
                &format!("{base}/100/mt_idle.hkx"),
                &format!("{base}/2000/mt_idle.hkx"),
            ],
        )];
        let table = ProviderTable::build(&facts);
        let dar = crate::game::builtin()[0]
            .domains
            .iter()
            .find(|d| d.id == "dar")
            .unwrap()
            .clone();
        let r = evaluate(&dar, Origin::Pack, &table, &Priorities(vec![0]), &|_| None);
        let c = &r.chains[0];
        assert_eq!(
            r.participants[c.stack[1] as usize].label, "2000",
            "the larger number wins"
        );
        assert_eq!(c.row(1), b"w");
        assert_eq!(
            lookup(b"[General]\nPriority = 7\n", "General.priority").and_then(|v| number(&v)),
            Some(7.0)
        );
    }
}
