//! Decoration that travels with the project (ADR 0008): colours, logical groups, collapsed separators — keyed by
//! entry **name**, so it survives a rebuild of the book — and the monolith's logical-group marker codec for
//! `modlist.txt` exports (`name ⟦MLT-LG:v1:<id>:<k>/<n>⟧`, v559/v566).
//!
//! Markers are only for explicit exports and imports of a project-flavoured list: MO2's own `modlist.txt` must
//! keep plain folder names. Import is strict, as in the monolith: any structural problem is an issue, nothing is
//! repaired silently.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use crate::canon::{EntryKind, Modlist};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decor {
    /// Entry name → colour.
    #[serde(default)]
    pub colors: BTreeMap<String, u32>,
    #[serde(default)]
    pub groups: Vec<GroupDef>,
    /// Collapsed separators (names).
    #[serde(default)]
    pub collapsed: Vec<String>,
    /// Muted entries (names): kept in the analysis, hidden from views.
    #[serde(default)]
    pub muted: Vec<String>,
}

impl Decor {
    pub fn is_empty(&self) -> bool {
        self.colors.is_empty() && self.groups.is_empty() && self.collapsed.is_empty() && self.muted.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupDef {
    /// Stable id (marker token).
    pub id: String,
    pub name: String,
    /// Member names in canon order.
    pub members: Vec<String>,
    #[serde(default = "yes")]
    pub collapsed: bool,
}

fn yes() -> bool {
    true
}

/// Why a piece of decoration was not applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "issue", rename_all = "kebab-case")]
pub enum DecorIssue {
    UnknownEntry { name: String },
    GroupRejected { id: String, reason: String },
}

const OPEN: &str = "⟦MLT-LG:v1:";
const CLOSE: char = '⟧';

/// A membership marker found in a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Marker {
    pub group_id: String,
    pub ordinal: u32,
    pub count: u32,
    /// Ordinal and count make sense (1 ≤ k ≤ n, n ≥ 2, id not empty).
    pub valid: bool,
    /// More than one marker on the line (the last one counts).
    pub duplicates: usize,
}

/// `⟦MLT-LG:v1:<id>:<k>/<n>⟧`; the id is percent-encoded like `encodeURIComponent`.
pub fn format_marker(group_id: &str, ordinal: u32, count: u32) -> String {
    format!("{OPEN}{}:{ordinal}/{count}{CLOSE}", encode(group_id))
}

/// Splits a name into the clean name and its last marker.
pub fn parse_marker(line: &str) -> (String, Option<Marker>) {
    let mut found: Vec<(usize, usize, Marker)> = Vec::new();
    let mut from = 0;
    while let Some(i) = line[from..].find(OPEN) {
        let start = from + i;
        let body_start = start + OPEN.len();
        let Some(j) = line[body_start..].find(CLOSE) else {
            break;
        };
        let body = &line[body_start..body_start + j];
        let end = body_start + j + CLOSE.len_utf8();
        if let Some(m) = parse_body(body) {
            found.push((start, end, m));
        }
        from = end;
    }
    let Some((start, end, mut m)) = found.pop() else {
        return (line.to_string(), None);
    };
    m.duplicates = found.len();
    let clean = format!("{}{}", &line[..start], &line[end..])
        .trim_end()
        .to_string();
    (clean, Some(m))
}

fn parse_body(body: &str) -> Option<Marker> {
    let (id, frac) = body.rsplit_once(':')?;
    let (k, n) = frac.split_once('/')?;
    let (ordinal, count) = (k.parse::<u32>().ok()?, n.parse::<u32>().ok()?);
    if id.is_empty() || id.contains(':') {
        return None;
    }
    let group_id = decode(id)?;
    Some(Marker {
        valid: !group_id.is_empty() && count >= 2 && ordinal >= 1 && ordinal <= count,
        group_id,
        ordinal,
        count,
        duplicates: 0,
    })
}

fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.trim().bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// A structural problem of markers in an imported list (monolith v559 validation).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "issue", rename_all = "kebab-case")]
pub enum MarkerIssue {
    InvalidMarker { row: usize },
    DuplicateMarkerOnRow { row: usize },
    MarkerOnNonMod { row: usize },
    MemberCountMismatch { group: String },
    MissingOrExtraMember { group: String },
    DuplicateOrdinal { group: String },
    NonContiguousGroup { group: String },
    OrdinalOrderMismatch { group: String },
}

/// Strips markers from a list; returns the clean list and the groups they describe, or every issue found.
pub fn strip_markers(list: &Modlist) -> Result<(Modlist, Vec<GroupDef>), Vec<MarkerIssue>> {
    let mut clean = list.clone();
    let mut issues = Vec::new();
    let mut by_group: HashMap<String, Vec<(usize, Marker)>> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for (row, e) in clean.entries.iter_mut().enumerate() {
        let (name, marker) = parse_marker(&e.name);
        let Some(m) = marker else { continue };
        e.name = name;
        if e.kind != EntryKind::Mod {
            issues.push(MarkerIssue::MarkerOnNonMod { row });
        }
        if !m.valid {
            issues.push(MarkerIssue::InvalidMarker { row });
        }
        if m.duplicates > 0 {
            issues.push(MarkerIssue::DuplicateMarkerOnRow { row });
        }
        if !by_group.contains_key(&m.group_id) {
            order.push(m.group_id.clone());
        }
        by_group.entry(m.group_id.clone()).or_default().push((row, m));
    }
    let mut groups = Vec::new();
    for (n, id) in order.into_iter().enumerate() {
        let rows = &by_group[&id];
        let count = rows[0].1.count;
        if rows.iter().any(|r| r.1.count != count) {
            issues.push(MarkerIssue::MemberCountMismatch { group: id.clone() });
        } else if rows.len() != count as usize {
            issues.push(MarkerIssue::MissingOrExtraMember { group: id.clone() });
        }
        let mut ords: Vec<u32> = rows.iter().map(|r| r.1.ordinal).collect();
        if rows.windows(2).any(|w| w[1].0 != w[0].0 + 1) {
            issues.push(MarkerIssue::NonContiguousGroup { group: id.clone() });
        }
        if rows.windows(2).any(|w| w[1].1.ordinal <= w[0].1.ordinal) {
            issues.push(MarkerIssue::OrdinalOrderMismatch { group: id.clone() });
        }
        ords.sort_unstable();
        if ords.windows(2).any(|w| w[0] == w[1]) {
            issues.push(MarkerIssue::DuplicateOrdinal { group: id.clone() });
        }
        groups.push(GroupDef {
            name: format!("Logical Group {}", n + 1),
            members: rows.iter().map(|r| clean.entries[r.0].name.clone()).collect(),
            collapsed: true,
            id,
        });
    }
    if issues.is_empty() {
        Ok((clean, groups))
    } else {
        Err(issues)
    }
}

/// Appends markers of `groups` to member names (members missing from the list are skipped).
pub fn encode_markers(list: &Modlist, groups: &[GroupDef]) -> Modlist {
    let mut out = list.clone();
    let index: HashMap<String, usize> = out
        .entries
        .iter()
        .enumerate()
        .map(|(i, e)| (e.name.to_lowercase(), i))
        .collect();
    for g in groups {
        let n = g.members.len() as u32;
        for (k, m) in g.members.iter().enumerate() {
            if let Some(&i) = index.get(&m.to_lowercase()) {
                let e = &mut out.entries[i];
                e.name = format!("{} {}", e.name.trim(), format_marker(&g.id, k as u32 + 1, n));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canon::Entry;

    fn list(names: &[&str]) -> Modlist {
        Modlist::new(
            names
                .iter()
                .map(|n| Entry {
                    name: n.to_string(),
                    kind: if n.contains("_separator") {
                        EntryKind::Separator
                    } else {
                        EntryKind::Mod
                    },
                    enabled: true,
                })
                .collect(),
        )
    }

    #[test]
    fn marker_round_trip_matches_monolith_format() {
        assert_eq!(format_marker("group v566", 1, 2), "⟦MLT-LG:v1:group%20v566:1/2⟧");
        let (clean, m) = parse_marker("SkyUI ⟦MLT-LG:v1:group%20v566:2/3⟧");
        assert_eq!(clean, "SkyUI");
        let m = m.unwrap();
        assert_eq!(
            (m.group_id.as_str(), m.ordinal, m.count, m.valid),
            ("group v566", 2, 3, true)
        );
        assert_eq!(parse_marker("Plain").1, None);
        assert!(!parse_marker("X ⟦MLT-LG:v1:g:3/2⟧").1.unwrap().valid);
    }

    #[test]
    fn strip_and_encode() {
        let groups = vec![GroupDef {
            id: "g1".into(),
            name: "Pack".into(),
            members: vec!["b".into(), "c".into()],
            collapsed: true,
        }];
        let plain = list(&["A_separator", "a", "b", "c", "d"]);
        let marked = encode_markers(&plain, &groups);
        assert_eq!(marked.entries[2].name, "b ⟦MLT-LG:v1:g1:1/2⟧");
        let (clean, back) = strip_markers(&marked).unwrap();
        assert_eq!(clean, plain);
        assert_eq!(back[0].members, groups[0].members);
    }

    #[test]
    fn strict_validation_reports_without_repair() {
        let bad = list(&[
            "a ⟦MLT-LG:v1:g:1/2⟧",
            "x",
            "b ⟦MLT-LG:v1:g:2/2⟧",
            "c ⟦MLT-LG:v1:h:1/3⟧",
            "d ⟦MLT-LG:v1:h:1/3⟧",
            "S_separator ⟦MLT-LG:v1:k:1/2⟧",
        ]);
        let issues = strip_markers(&bad).unwrap_err();
        assert!(issues.contains(&MarkerIssue::NonContiguousGroup { group: "g".into() }));
        assert!(issues.contains(&MarkerIssue::MissingOrExtraMember { group: "h".into() }));
        assert!(issues.contains(&MarkerIssue::DuplicateOrdinal { group: "h".into() }));
        assert!(issues.contains(&MarkerIssue::MarkerOnNonMod { row: 5 }));
    }
}
