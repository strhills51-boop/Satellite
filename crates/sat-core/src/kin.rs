//! Kin: how two mods are related — one origin in another form, or one covering another (ADR 0023). @sat:kin
//!
//! Owns: the kinds of kinship (copy, version, variant, addon, translation) and "covers", the name base a mod's
//! folder name reduces to, versions read from a source or a name and their order, the rule that classifies a pair
//! from its facts (origin, surface both ways, bytes, versions, plugins) with the evidence it rests on, families and
//! usurpers.
//! Not here: gathering the pair facts from the table and the content hashes (`@sat:cli.kin`); neighbours by records
//! and sides of the game (ADR 0023, step 2).
//!
//! A link is never one score: it is a kind, a direction where the kind has one (older → newer, addon → base,
//! translation → original, covering → covered) and the shares it was read from. The thresholds are a first version
//! (ADR 0023), tuned on the owner's build.

use std::cmp::Ordering;

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

// @sat:kin.kind
/// How mod `a` is related to mod `b`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// The same bytes under another name (a re-upload, a renamed copy): one of them on.
    Copy,
    /// The same origin, another version: `a` older, `b` newer (when known). One of them on.
    Version,
    /// The same origin and version, another option (2K / 4K, "– BOS"): the same paths, other bytes. One of them on.
    Variant,
    /// `a` adds to `b` (an addon, a patch, a fix of the same origin — its plugin needs `b`'s, or it overlays part
    /// of `b`, or its name says so): both on, `a` after `b`.
    Addon,
    /// From one mod page, side by side: few or no shared files, neither needs the other (a pack's other animations,
    /// an example config, a preset beside a replacer).
    Sibling,
    /// `a` translates `b`: the same plugin by name, few assets; `a`'s plugin instead of `b`'s.
    Translation,
    /// Not kin: `a` covers most of `b` with its own bytes (a pack over a single mod).
    Covers,
}

impl Kind {
    /// Kin of one family (`Covers` is not).
    pub fn kin(self) -> bool {
        self != Kind::Covers
    }
    /// Only one of the two should be on: two of them on is a usurper.
    pub fn exclusive(self) -> bool {
        matches!(self, Kind::Copy | Kind::Version | Kind::Variant)
    }
    pub fn label(self) -> &'static str {
        match self {
            Kind::Copy => "копия",
            Kind::Version => "версия",
            Kind::Variant => "вариант",
            Kind::Addon => "дополнение",
            Kind::Sibling => "с одной страницы",
            Kind::Translation => "перевод",
            Kind::Covers => "перекрывает",
        }
    }
}

// @sat:kin.facts
/// What is known of one mod for kinship.
#[derive(Debug, Clone, Default)]
pub struct Side {
    pub name: String,
    pub enabled: bool,
    /// Nexus mod id (`meta.ini`).
    pub mod_id: Option<u64>,
    /// The version the source records (`meta.ini`); else none — the name may still carry one.
    pub version: Option<String>,
    /// The archive it was installed from (`meta.ini`).
    pub file: Option<String>,
    /// Bytes of the game's files (no `meta.ini`, fomod, readme, pictures in the root).
    pub bytes: u64,
    /// Plugin file names at the root, lower case.
    pub plugins: Vec<String>,
    /// Masters of those plugins, lower case.
    pub masters: Vec<String>,
}

/// What two mods share, from `a`'s side: the paths both provide.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Surface {
    pub files: u32,
    /// `a`'s bytes on the shared paths.
    pub bytes_a: u64,
    /// `b`'s bytes on the shared paths.
    pub bytes_b: u64,
    /// Of `bytes_a`: the same bytes in `b`, other bytes, not compared.
    pub same: u64,
    pub differ: u64,
}

/// What a link rests on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Why {
    /// The same Nexus mod, confirmed by an archive or folder name (`Name-ID-version-…`).
    NexusId(u64),
    /// The same name once versions, dates, Nexus numbers and copy marks are gone.
    Name,
    /// Names close (one's words in the other's, or half of them shared).
    NameNear,
    /// Versions `a` → `b`.
    Versions(String, String),
    /// A plugin of `a` has a plugin of `b` for a master.
    Masters,
    /// A plugin of the same name.
    PluginName,
    /// The bytes were compared.
    Bytes,
    /// The name says so (patch, addon, fix, compatibility).
    Marked,
}

/// Name bases too generic to tell an origin (a browser's download folder, an archive's default name).
const GENERIC: &[&str] = &[
    "drive download",
    "new folder",
    "data",
    "archive",
    "files",
    "main",
    "main file",
    "mod",
    "mods",
    "patch",
    "patches",
    "fix",
    "fixes",
    "update",
    "optional",
    "optional files",
    "textures",
    "meshes",
    "plugin",
    "plugins",
    "esp",
    "esl version",
    "test",
    "temp",
    "download",
    "downloads",
    "sse",
    "skyrim",
    "skse",
    "addon",
    "hotfix",
];

/// Words that mark a patch or an addon in a name.
const MARKS: &[&str] = &[
    "patch",
    "patches",
    "addon",
    "add",
    "fix",
    "fixes",
    "compat",
    "compatibility",
    "tweak",
    "tweaks",
    "plugin",
    "extension",
    "expansion",
    "integration",
    "replacer",
];

/// A Nexus id confirmed by an archive or folder name of the Nexus kind (`Name-12604-5-2-…`): MO2 also writes ids
/// it guessed or the owner picked by hand, and those join unrelated mods.
fn id_confirmed(s: &Side, id: u64) -> bool {
    let mark = format!("-{id}-");
    s.name.contains(&mark) || s.file.as_deref().is_some_and(|f| f.contains(&mark))
}

// @sat:kin.link
/// A link between two mods (indices into the caller's list), oriented by its kind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Link {
    pub a: u32,
    pub b: u32,
    pub kind: Kind,
    /// Origin and shares agree; otherwise "likely".
    pub sure: bool,
    /// `a → b` has a meaning for this kind (older → newer, addon → base, covering → covered).
    pub oriented: bool,
    /// Share of `a`'s bytes on paths `b` has too; of `b`'s on paths `a` has.
    pub cover_ab: f32,
    pub cover_ba: f32,
    /// Of the compared shared bytes, the share that is the same (`None` — none compared).
    pub same: Option<f32>,
    pub why: Vec<Why>,
}

// @sat:kin.name
/// The base a mod's folder name reduces to: lower case, without versions, dates, Nexus download numbers, edition
/// marks (SE / AE / SSE), copy marks (`(1)`, `copy`, a digit MO2 glued to the end) and punctuation.
pub fn base_name(name: &str) -> String {
    // a random token a downloader adds (`RHaV1OLNk1`): letters and digits, upper and lower case
    let random: Vec<String> = name
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| {
            // a digit inside the token (not only a trailing one) and two capitals: `Follower1` is a name
            let inner_digit = t
                .trim_end_matches(|c: char| c.is_ascii_digit())
                .chars()
                .any(|c| c.is_ascii_digit());
            t.len() >= 8
                && inner_digit
                && t.chars().filter(char::is_ascii_uppercase).count() >= 2
                && t.chars().any(|c| c.is_ascii_lowercase())
        })
        .map(str::to_lowercase)
        .collect();
    let mut words: Vec<&str> = Vec::new();
    let low = name.to_lowercase();
    for raw in low.split(|c: char| !c.is_alphanumeric() && c != '.') {
        let w = raw.trim_matches('.');
        // numbers, versions, dates, sizes (`2k`): anything that starts with a digit
        if w.is_empty() || w.starts_with(|c: char| c.is_ascii_digit()) || random.iter().any(|r| r == w) {
            continue;
        }
        // `v1.2`, `ver2`, `version`
        let num = w
            .trim_start_matches("version")
            .trim_start_matches("ver")
            .trim_start_matches('v');
        if num.len() < w.len() && num.chars().all(|c| c.is_ascii_digit() || c == '.') {
            continue;
        }
        // a digit MO2 glues to a repeated name (`follower1`, `se1`)
        let stem = w.trim_end_matches(|c: char| c.is_ascii_digit());
        let w = if w.len() - stem.len() <= 2 { stem } else { w };
        if matches!(
            w,
            "se" | "ae"
                | "sse"
                | "le"
                | "vr"
                | "v"
                | "copy"
                | "backup"
                | "bak"
                | "old"
                | "esl"
                | "esp"
                | "esm"
                | "full"
                | "version"
                | "jan"
                | "feb"
                | "mar"
                | "apr"
                | "may"
                | "jun"
                | "jul"
                | "aug"
                | "sep"
                | "sept"
                | "oct"
                | "nov"
                | "dec"
        ) {
            continue;
        }
        words.push(w);
    }
    words.join(" ")
}

/// Names close: every word of the shorter base in the longer, at least half the words shared, or the same first
/// two words (10 letters or more: `medieval markets …`, `utenlands nordic tents …`).
pub fn names_near(a: &str, b: &str) -> bool {
    let (wa, wb): (Vec<&str>, Vec<&str>) = (a.split(' ').collect(), b.split(' ').collect());
    if a.is_empty() || b.is_empty() || a.len().min(b.len()) < 5 {
        return false;
    }
    let (s, l) = if wa.len() <= wb.len() {
        (&wa, &wb)
    } else {
        (&wb, &wa)
    };
    let inter = s.iter().filter(|w| l.contains(w)).count();
    let union = wa.len() + wb.len() - inter;
    let lead = wa.len() >= 2 && wb.len() >= 2 && wa[..2] == wb[..2] && wa[0].len() + wa[1].len() >= 10;
    inter == s.len() || inter * 2 >= union || lead
}

// @sat:kin.version
/// The numbers of a version (`d1.0.2`, `v9.0`, `3.6.11`): none when it has no digits.
pub fn version_numbers(v: &str) -> Option<Vec<u64>> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in v.chars() {
        if c.is_ascii_digit() {
            cur.push(c);
        } else if !cur.is_empty() {
            if c != '.' && c != '_' && c != '-' {
                out.push(cur.parse().ok()?);
                cur.clear();
                break;
            }
            out.push(cur.parse().ok()?);
            cur.clear();
        }
    }
    if !cur.is_empty() {
        out.push(cur.parse().ok()?);
    }
    (!out.is_empty()).then_some(out)
}

/// A version written in a folder name: `v9.0`, `ver2`, `3.6.11` — a number with a dot or after `v`; years and
/// Nexus numbers are not versions.
pub fn version_in_name(name: &str) -> Option<String> {
    for raw in
        name.split(|c: char| c.is_whitespace() || c == '_' || c == '(' || c == ')' || c == '[' || c == ']')
    {
        let low = raw.to_lowercase();
        let t = low
            .trim_start_matches("version")
            .trim_start_matches("ver")
            .trim_start_matches('v');
        let after_v = t.len() < low.len();
        let ok = t.chars().next().is_some_and(|c| c.is_ascii_digit())
            && t.chars()
                .all(|c| c.is_ascii_digit() || c == '.' || c.is_ascii_lowercase())
            && (t.contains('.') || after_v)
            && !t.split('.').next().is_some_and(|x| x.len() >= 4);
        if ok {
            return Some(t.trim_end_matches(|c: char| c.is_ascii_lowercase()).to_string());
        }
    }
    None
}

/// Order of two versions by their numbers (a missing number is 0): `None` when either has none.
pub fn compare_versions(a: &str, b: &str) -> Option<Ordering> {
    let (x, y) = (version_numbers(a)?, version_numbers(b)?);
    let n = x.len().max(y.len());
    Some(
        (0..n)
            .map(|i| x.get(i).unwrap_or(&0).cmp(y.get(i).unwrap_or(&0)))
            .find(|o| o.is_ne())
            .unwrap_or(Ordering::Equal),
    )
}

fn version_of(s: &Side) -> Option<String> {
    s.version
        .clone()
        .filter(|v| version_numbers(v).is_some())
        .or_else(|| version_in_name(&s.name))
}

// @sat:kin.classify
/// The link between mods `ia` and `ib`, if any, oriented by its kind. `s` is the surface from `a`'s side.
pub fn classify(ia: u32, a: &Side, ib: u32, b: &Side, s: &Surface) -> Option<Link> {
    if a.bytes == 0 || b.bytes == 0 {
        return None;
    }
    let cover_ab = (s.bytes_a as f64 / a.bytes as f64).min(1.0) as f32;
    let cover_ba = (s.bytes_b as f64 / b.bytes as f64).min(1.0) as f32;
    let compared = s.same + s.differ;
    let same = (compared > 0).then(|| (s.same as f64 / compared as f64) as f32);
    let known = if s.bytes_a > 0 {
        compared as f64 / s.bytes_a as f64
    } else {
        0.0
    };
    // the smaller mod's share in the bigger one, the bigger's in the smaller
    let (contain, sim) = (cover_ab.max(cover_ba), cover_ab.min(cover_ba));

    let mut why = Vec::new();
    let (ba, bb) = (base_name(&a.name), base_name(&b.name));
    let generic = |x: &str| x.is_empty() || GENERIC.contains(&x);
    let name_same = !generic(&ba) && ba == bb;
    let near = !name_same && !generic(&ba) && !generic(&bb) && names_near(&ba, &bb);
    // one Nexus id, confirmed by the names of both, or of one when the names are close too
    let id_same = match (a.mod_id, b.mod_id) {
        (Some(x), Some(y)) if x == y => {
            let (ca, cb) = (id_confirmed(a, x), id_confirmed(b, x));
            (ca && cb) || ((ca || cb) && (name_same || near))
        }
        _ => false,
    };
    if let (true, Some(id)) = (id_same, a.mod_id) {
        why.push(Why::NexusId(id));
    }
    if name_same {
        why.push(Why::Name);
    } else if near {
        why.push(Why::NameNear);
    }
    let strong = id_same || name_same;
    let origin = strong || near;
    let masters_ab = a.masters.iter().any(|m| b.plugins.contains(m));
    let masters_ba = b.masters.iter().any(|m| a.plugins.contains(m));
    let plugin_name = a.plugins.iter().any(|p| b.plugins.contains(p));
    let link = |kind, sure, oriented, flip: bool, mut why: Vec<Why>| {
        let (x, y, cab, cba) = if flip {
            (ib, ia, cover_ba, cover_ab)
        } else {
            (ia, ib, cover_ab, cover_ba)
        };
        if compared > 0 {
            why.push(Why::Bytes);
        }
        Some(Link {
            a: x,
            b: y,
            kind,
            sure,
            oriented,
            cover_ab: cab,
            cover_ba: cba,
            same,
            why,
        })
    };

    // 1. a copy: nearly all of both is the other, byte for byte
    if contain >= 0.9 && sim >= 0.9 && same.is_some_and(|x| x >= 0.95) && known >= 0.5 {
        return link(Kind::Copy, known >= 0.9, false, false, why);
    }
    let small_is_a = a.bytes <= b.bytes;
    let marked = {
        let small = if small_is_a { &ba } else { &bb };
        small.split(' ').any(|w| MARKS.contains(&w))
    };
    if origin {
        let (va, vb) = (version_of(a), version_of(b));
        let versions_differ =
            matches!((&va, &vb), (Some(x), Some(y)) if compare_versions(x, y).is_some_and(Ordering::is_ne));
        // another version or another option: each holds much of the other — or one name, other versions (a new
        // version that grew)
        if (contain >= 0.5 && sim >= 0.5) || (contain >= 0.5 && name_same && versions_differ) {
            if let (Some(x), Some(y)) = (&va, &vb) {
                why.push(Why::Versions(x.clone(), y.clone()));
                match compare_versions(x, y) {
                    Some(Ordering::Less) => return link(Kind::Version, strong, true, false, why),
                    Some(Ordering::Greater) => return link(Kind::Version, strong, true, true, why),
                    _ => {
                        return link(
                            Kind::Variant,
                            strong && same.is_some_and(|x| x < 0.9),
                            false,
                            false,
                            why,
                        );
                    }
                }
            }
            return link(Kind::Version, false, false, false, why);
        }
        let (small, big) = if small_is_a { (a, b) } else { (b, a) };
        let masters = if small_is_a { masters_ab } else { masters_ba };
        if masters {
            why.push(Why::Masters);
        }
        if marked {
            why.push(Why::Marked);
        }
        // the smaller one lies over part of the other: an overlay of the base (a replacer, an option patch)
        if contain >= 0.5 {
            return link(Kind::Addon, strong, true, !small_is_a, why);
        }
        // the smaller one is mostly its own files
        if plugin_name && small.bytes * 10 <= big.bytes && !masters {
            why.push(Why::PluginName);
            return link(Kind::Translation, false, true, !small_is_a, why);
        }
        if masters || (marked && origin) {
            return link(Kind::Addon, strong || masters, true, !small_is_a, why);
        }
        if strong {
            return link(Kind::Sibling, id_same, false, false, why);
        }
        // names only close and few shared files: not enough to call it kin
        return None;
    }
    // 3. no shared origin: one covers most of the other (at least 1 MiB of it)
    let small = a.bytes.min(b.bytes);
    if contain >= 0.9 && small >= 1 << 20 {
        // the covering one is the bigger
        return link(Kind::Covers, true, true, small_is_a, why);
    }
    None
}

// @sat:kin.families
/// A family: mods linked by kin (not by `Covers`), its base and who of it is on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Family {
    /// The base: what addons lead to; among versions the newest.
    pub root: u32,
    pub members: Vec<u32>,
    /// Links inside the family.
    pub links: Vec<Link>,
    /// Pairs of members both on while only one should be (copy, version, variant).
    pub usurpers: Vec<(u32, u32, Kind)>,
}

/// Families from the links: components of kin links, a base for each, usurpers.
pub fn families(links: &[Link], sides: &[Side]) -> Vec<Family> {
    let mut parent: FxHashMap<u32, u32> = FxHashMap::default();
    fn find(p: &mut FxHashMap<u32, u32>, x: u32) -> u32 {
        let mut r = x;
        while let Some(&q) = p.get(&r) {
            if q == r {
                break;
            }
            r = q;
        }
        p.insert(x, r);
        r
    }
    for l in links.iter().filter(|l| l.kind.kin()) {
        parent.entry(l.a).or_insert(l.a);
        parent.entry(l.b).or_insert(l.b);
        let (x, y) = (find(&mut parent, l.a), find(&mut parent, l.b));
        if x != y {
            parent.insert(x, y);
        }
    }
    let mut groups: FxHashMap<u32, Family> = FxHashMap::default();
    let keys: Vec<u32> = parent.keys().copied().collect();
    for m in keys {
        let r = find(&mut parent, m);
        groups
            .entry(r)
            .or_insert_with(|| Family {
                root: r,
                members: Vec::new(),
                links: Vec::new(),
                usurpers: Vec::new(),
            })
            .members
            .push(m);
    }
    for l in links.iter().filter(|l| l.kind.kin()) {
        let r = find(&mut parent, l.a);
        if let Some(f) = groups.get_mut(&r) {
            f.links.push(l.clone());
        }
    }
    let mut out: Vec<Family> = groups.into_values().collect();
    for f in &mut out {
        f.members.sort_unstable();
        // the base: led to by addons and translations, not older than another, on, the biggest
        let score = |m: u32| {
            let into = f
                .links
                .iter()
                .filter(|l| l.b == m && matches!(l.kind, Kind::Addon | Kind::Translation))
                .count();
            let outof = f
                .links
                .iter()
                .filter(|l| {
                    l.a == m
                        && l.oriented
                        && matches!(l.kind, Kind::Addon | Kind::Translation | Kind::Version)
                })
                .count();
            let s = &sides[m as usize];
            (into, std::cmp::Reverse(outof), s.enabled, s.bytes)
        };
        f.root = f
            .members
            .iter()
            .copied()
            .max_by_key(|&m| score(m))
            .unwrap_or(f.root);
        f.usurpers = f
            .links
            .iter()
            .filter(|l| l.kind.exclusive() && sides[l.a as usize].enabled && sides[l.b as usize].enabled)
            .map(|l| (l.a, l.b, l.kind))
            .collect();
    }
    out.sort_by_key(|f| std::cmp::Reverse(f.members.iter().map(|&m| sides[m as usize].bytes).sum::<u64>()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn side(name: &str, bytes: u64) -> Side {
        Side {
            name: name.into(),
            enabled: true,
            bytes,
            ..Side::default()
        }
    }

    #[test]
    fn names_lose_versions_dates_nexus_numbers_and_copy_marks() {
        assert_eq!(
            base_name("Billyy's SLAL Petite Animations 8.1"),
            base_name("Billyy's SLAL Petite Animations 8.1 (1)")
        );
        assert_eq!(
            base_name("KuWa Heel Sound-148967-1-0-17460965361"),
            "kuwa heel sound"
        );
        assert_eq!(
            base_name("Papyrus Extender 22854 6.5.1 2026-08-25T23-16Z RHaV1OLNk1"),
            "papyrus extender"
        );
        assert_eq!(base_name("mihoyo1"), base_name("mihoyo"));
        assert_eq!(base_name("Seol Yi Chan Follower1"), "seol yi chan follower");
        assert_eq!(
            base_name("PamaDeadlyFurniture_V3.5.0_SE_AE"),
            "pamadeadlyfurniture"
        );
        assert_eq!(base_name("PamaDeadlyFurniture_V 2025 dec"), "pamadeadlyfurniture");
        assert!(names_near(
            &base_name("ALT - Crates"),
            &base_name("ALT - Crates - BOS")
        ));
        assert!(!names_near(
            &base_name("Supermutants 2.0"),
            &base_name("Heels Sound 1.6 SSE-AE")
        ));
    }

    #[test]
    fn versions_are_read_and_ordered() {
        assert_eq!(
            version_in_name("SLAL_Billyy_Petite_Animations v9.0 SE").as_deref(),
            Some("9.0")
        );
        assert_eq!(
            version_in_name("Flufy Fox Poser SE 3.3.2").as_deref(),
            Some("3.3.2")
        );
        assert_eq!(
            version_in_name("Ork Baka ABC + HC Addon ver2 (2)").as_deref(),
            Some("2")
        );
        assert_eq!(version_in_name("PamaDeadlyFurniture_V 2025 dec"), None);
        assert_eq!(compare_versions("8.1", "9.0"), Some(Ordering::Less));
        assert_eq!(compare_versions("d1.0.2", "1.0.2"), Some(Ordering::Equal));
        assert_eq!(compare_versions("3.6.11", "3.6"), Some(Ordering::Greater));
    }

    #[test]
    fn a_pair_is_read_by_kind_with_its_direction() {
        let surf = |a: u64, b: u64, same: u64, differ: u64| Surface {
            files: 10,
            bytes_a: a,
            bytes_b: b,
            same,
            differ,
        };
        // a re-upload: the same bytes, unrelated names
        let l = classify(
            0,
            &side("BLUE3.0", 100 << 20),
            1,
            &side("布鲁阿凯乌1", 104 << 20),
            &surf(99 << 20, 99 << 20, 99 << 20, 0),
        )
        .unwrap();
        assert_eq!(l.kind, Kind::Copy);
        // versions: older → newer, whichever comes first
        let (v8, v9) = (
            side("Billyy's SLAL Petite Animations 8.1", 800 << 20),
            side("SLAL_Billyy_Petite_Animations v9.0 SE", 850 << 20),
        );
        let s = surf(700 << 20, 720 << 20, 300 << 20, 400 << 20);
        let l = classify(0, &v9, 1, &v8, &s).unwrap();
        assert_eq!(
            (l.kind, l.oriented, l.a, l.b),
            (Kind::Version, true, 1, 0),
            "8.1 → 9.0"
        );
        // an addon of the same Nexus mod: mostly its own files, toward the base
        let mut base = side(
            "Utenlands Nordic Tents - BOS Color Variance-7-1-0-1700000000",
            300 << 20,
        );
        let mut add = side(
            "Utenlands Nordic Tents - Campfire Addon-7-1-0-1700000001",
            30 << 20,
        );
        base.mod_id = Some(7);
        add.mod_id = Some(7);
        let l = classify(0, &base, 1, &add, &surf(2 << 20, 2 << 20, 2 << 20, 0)).unwrap();
        assert_eq!((l.kind, l.a, l.b), (Kind::Addon, 1, 0));
        // unrelated, one covers the other
        let l = classify(
            0,
            &side("Big Texture Pack", 900 << 20),
            1,
            &side("Riverwood Fences", 20 << 20),
            &surf(19 << 20, 19 << 20, 0, 19 << 20),
        )
        .unwrap();
        assert_eq!((l.kind, l.a, l.b), (Kind::Covers, 0, 1));
        // unrelated and little shared: nothing
        assert!(
            classify(
                0,
                &side("A mod", 100 << 20),
                1,
                &side("Other", 100 << 20),
                &surf(1 << 20, 1 << 20, 0, 0)
            )
            .is_none()
        );
    }

    #[test]
    fn a_guessed_nexus_id_a_generic_name_and_an_overlay_are_read_right() {
        let surf = |a: u64, b: u64, differ: u64| Surface {
            files: 3,
            bytes_a: a,
            bytes_b: b,
            same: 0,
            differ,
        };
        // MO2 wrote the same id to unrelated mods: no name confirms it, the names differ — not kin
        let mut hair = side("ApachiiSkyHairFemale_v_1_5_1", 1100 << 20);
        let mut tats = side("Rutah-nordic", 320 << 20);
        hair.mod_id = Some(2014);
        hair.file = Some("ApachiiSkyHairFemale-2014-1-5-1.7z".into());
        tats.mod_id = Some(2014);
        assert!(classify(0, &hair, 1, &tats, &Surface::default()).is_none());
        // a browser's download folders share a generic name, not an origin
        let d1 = side("drive-download-20260830T115523Z-1-0041", 3000 << 20);
        let d2 = side("drive-download-20260625T020010Z-3-001", 900 << 20);
        assert!(classify(0, &d1, 1, &d2, &Surface::default()).is_none());
        // a small patch lying over part of the main mod: an addon after it, not a version
        let main = side("Medieval Markets ESL", 900 << 20);
        let patch = side("Medieval Markets - ALT Crates Patch", 9 << 20);
        let l = classify(0, &main, 1, &patch, &surf(9 << 20, 9 << 20, 9 << 20)).unwrap();
        assert_eq!((l.kind, l.a, l.b), (Kind::Addon, 1, 0));
        // two files of one page that share nothing: side by side
        let mut a = side("Graceful woman Jump_legs apart-163117-1-0-1761839000", 1 << 20);
        let mut b = side("Cute girl jump_legs apart-163117-1-0-1761839700", 1 << 20);
        a.mod_id = Some(163117);
        b.mod_id = Some(163117);
        assert_eq!(
            classify(0, &a, 1, &b, &Surface::default()).unwrap().kind,
            Kind::Sibling
        );
    }

    #[test]
    fn families_have_a_base_and_usurpers() {
        let mut sides = vec![side("X 1.0", 100), side("X 2.0", 110), side("X Patch", 10)];
        sides[0].enabled = true;
        let links = vec![
            Link {
                a: 0,
                b: 1,
                kind: Kind::Version,
                sure: true,
                oriented: true,
                cover_ab: 0.9,
                cover_ba: 0.8,
                same: None,
                why: vec![],
            },
            Link {
                a: 2,
                b: 1,
                kind: Kind::Addon,
                sure: true,
                oriented: true,
                cover_ab: 0.1,
                cover_ba: 0.0,
                same: None,
                why: vec![],
            },
        ];
        let f = families(&links, &sides);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].root, 1, "the newest, the addon's base");
        assert_eq!(f[0].usurpers, vec![(0, 1, Kind::Version)], "two versions on");
    }
}
