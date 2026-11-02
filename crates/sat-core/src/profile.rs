//! Content profile: what a mod is made of, in the monolith's terms (v602–v609). @sat:profile
//!
//! Owns: counts of a mod's files by content family, path domain and extension, each split by origin (loose /
//! inside a BSA/BA2), path shape (depth, long, junk), and what they add up to — the primary kind, coverage, the
//! package shape, cargo signals and profile labels.
//! Not here: the scan (`@sat:scan`), the metric catalog that exposes these (`@sat:metrics`), tag rules
//! (`@sat:tag-rules`).
//!
//! Built from facts when the table is built (`@sat:table.summary`), so facts saved before this module get a
//! profile without a rescan. Thresholds and word lists are the monolith's (S01 §4) and live here as tables.
//! Differences from the monolith, on purpose: `.rar` counts as a nested archive and `.bik` (the game's video) as
//! video; no 128-key cap on extensions, so there are no overflow labels; a logical group's profile keeps the
//! origins of its members (the monolith's group aggregate lost them).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Origin of an observation: a loose file or an archive entry.
pub const LOOSE: usize = 0;
pub const ARCHIVED: usize = 1;

// @sat:profile.families
/// Content families (the monolith's v602 order).
pub const FAMILIES: [(&str, &[&str]); 9] = [
    ("textures", &["dds"]),
    ("meshes", &["nif"]),
    ("scripts", &["pex"]),
    ("animations", &["hkx"]),
    ("native", &["dll", "dllplugin"]),
    ("game_archives", &["bsa", "ba2"]),
    ("nested_archives", &["7z", "zip", "tar", "rar"]),
    (
        "authoring",
        &[
            "psd",
            "pdn",
            "xcf",
            "fla",
            "fbx",
            "obj",
            "cpp",
            "h",
            "headeronly",
            "pas",
            "py",
            "pyw",
            "psc",
            "hlsl",
            "hlsli",
            "frag",
            "vert",
            "lua",
            "as",
            "vbs",
            "ps1",
            "bat",
            "csh",
            "fish",
        ],
    ),
    (
        "debug",
        &["pdb", "pyc", "cl_cache", "idb", "log", "out", "code-workspace"],
    ),
];
/// Families that are game content (the composition is over these).
const RUNTIME_KINDS: usize = 5;
/// Share at which a kind dominates (v605); native code has no threshold — its presence is the label.
const DOMINANT_AT: [f64; 4] = [0.60, 0.50, 0.40, 0.50];
/// Cargo families with the confidence each signal carries (v604).
const CARGO: [(usize, &str, f64); 3] = [
    (6, "nested-archives", 0.85),
    (8, "debug-build-artifacts", 0.75),
    (7, "authoring-sources", 0.65),
];

// @sat:profile.domains
/// Path domains by first folder (v607 order).
pub const DOMAINS: [&str; 19] = [
    "root",
    "textures",
    "meshes",
    "scripts",
    "interface",
    "sound",
    "music",
    "video",
    "strings",
    "skse",
    "seq",
    "lod",
    "config",
    "fomod",
    "tools",
    "docs",
    "source",
    "data",
    "other",
];
const D_ROOT: usize = 0;
const D_DATA: usize = 17;
const D_OTHER: usize = 18;
/// Domains the game reads (v608), and those that only come with the package.
const RUNTIME_DOMAINS: std::ops::Range<usize> = 1..13;
const SUPPORT_DOMAINS: std::ops::Range<usize> = 13..17;

/// The domain of a path's first folder.
fn domain(first: &str) -> usize {
    match first {
        "textures" => 1,
        "meshes" => 2,
        "scripts" => 3,
        "interface" => 4,
        "sound" | "sounds" | "voice" => 5,
        "music" => 6,
        "video" | "videos" => 7,
        "strings" => 8,
        "skse" => 9,
        "seq" => 10,
        "lodsettings" | "terrain" | "lod" | "distantlod" => 11,
        "config" | "configs" | "ini" | "settings" => 12,
        "fomod" => 13,
        "tools" | "tool" => 14,
        "docs" | "doc" | "documentation" | "readme" => 15,
        "source" | "src" | "sources" => 16,
        "data" => D_DATA,
        _ => D_OTHER,
    }
}

const DEEP_SEGMENTS: u32 = 8;
const LONG_CHARS: usize = 180;
const JUNK_FILES: [&str; 5] = [".ds_store", "thumbs.db", "desktop.ini", "ehthumbs.db", "icon\r"];
const JUNK_DIRS: [&str; 2] = ["__macosx", ".git"];

// @sat:profile.presentation
/// Presentation formats (v609): kind, extensions, the domains where they belong.
const PRESENTATION: [(&str, &[&str], &[usize]); 4] = [
    (
        "localization",
        &["strings", "dlstrings", "ilstrings", "po", "mo", "pot"],
        &[8, 4],
    ),
    ("interface", &["swf", "gfx", "html", "css", "svg"], &[4]),
    (
        "audio",
        &[
            "fuz", "xwm", "wav", "lip", "mp3", "ogg", "flac", "wem", "bnk", "m4a", "aac", "opus",
        ],
        &[5, 6],
    ),
    ("video", &["mp4", "webm", "bik"], &[7]),
];

/// Counts behind a mod's profile. `[loose, archived]` pairs throughout.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentProfile {
    pub observed: [u32; 2],
    pub families: [[u32; 2]; 9],
    pub domains: [[u32; 2]; 19],
    /// By extension (lower case, `""` — none).
    pub extensions: BTreeMap<String, [u32; 2]>,
    pub max_depth: u32,
    pub depth_sum: u64,
    pub deep: u32,
    pub long: u32,
    pub junk: u32,
    /// Presentation formats by kind, and how many of them sit outside their domain.
    pub presentation: [u32; 4],
    pub misplaced: [u32; 4],
    /// Voice formats (`.fuz`/`.lip`) under a sound folder; flash (`.swf`/`.gfx`) under interface.
    pub voice: u32,
    pub flash: u32,
    pub plugins: u32,
}

/// One folder's place, worked out once for all its files.
#[derive(Debug, Clone, Copy)]
pub struct DirShape {
    domain: usize,
    /// Segments of the folder (0 — the mod root).
    segments: u32,
    junk: bool,
    len: usize,
}

impl DirShape {
    /// `rel` — the folder relative to the mod root, `/`-separated, `""` for the root.
    pub fn of(rel: &str) -> Self {
        let rel = rel.trim_matches('/');
        if rel.is_empty() {
            return Self {
                domain: D_ROOT,
                segments: 0,
                junk: false,
                len: 0,
            };
        }
        let lower = rel.to_lowercase();
        let mut segs = lower.split('/').filter(|s| !s.is_empty());
        let first = segs.next().unwrap_or("");
        let junk = JUNK_DIRS.contains(&first) || segs.clone().any(|s| JUNK_DIRS.contains(&s));
        Self {
            domain: domain(first),
            segments: 1 + segs.count() as u32,
            junk,
            len: rel.len(),
        }
    }
}

fn ext_of(name: &str) -> String {
    match name.rfind('.') {
        Some(i) if i + 1 < name.len() => name[i + 1..].to_lowercase(),
        _ => String::new(),
    }
}

fn pct(part: u32, whole: u32) -> f64 {
    if whole == 0 {
        return 0.0;
    }
    (f64::from(part) * 10_000.0 / f64::from(whole)).round() / 100.0
}

// @sat:profile.build
impl ContentProfile {
    /// Counts one file named `name` in a folder of shape `dir`.
    pub fn observe(&mut self, dir: DirShape, name: &str, origin: usize) {
        let ext = ext_of(name);
        self.observed[origin] += 1;
        let depth = dir.segments + 1;
        self.max_depth = self.max_depth.max(depth);
        self.depth_sum += u64::from(depth);
        if depth >= DEEP_SEGMENTS {
            self.deep += 1;
        }
        if dir.len + 1 + name.len() >= LONG_CHARS {
            self.long += 1;
        }
        if dir.junk || JUNK_FILES.iter().any(|j| name.eq_ignore_ascii_case(j)) {
            self.junk += 1;
        }
        self.domains[dir.domain][origin] += 1;
        if let Some(f) = FAMILIES.iter().position(|(_, exts)| exts.contains(&ext.as_str())) {
            self.families[f][origin] += 1;
        }
        if let Some(k) = PRESENTATION
            .iter()
            .position(|(_, exts, _)| exts.contains(&ext.as_str()))
        {
            self.presentation[k] += 1;
            if !PRESENTATION[k].2.contains(&dir.domain) {
                self.misplaced[k] += 1;
            }
            if dir.domain == 5 && matches!(ext.as_str(), "fuz" | "lip") {
                self.voice += 1;
            }
            if dir.domain == 4 && matches!(ext.as_str(), "swf" | "gfx") {
                self.flash += 1;
            }
        }
        self.extensions.entry(ext).or_default()[origin] += 1;
    }

    /// Counts an archive entry by its full path (`a/b/c.dds`).
    pub fn observe_path(&mut self, path: &str, origin: usize) {
        let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
        self.observe(DirShape::of(dir), name, origin);
    }

    /// Adds another profile (a logical group's members).
    pub fn merge(&mut self, o: &ContentProfile) {
        for i in 0..2 {
            self.observed[i] += o.observed[i];
            for (a, b) in self.families.iter_mut().zip(&o.families) {
                a[i] += b[i];
            }
            for (a, b) in self.domains.iter_mut().zip(&o.domains) {
                a[i] += b[i];
            }
        }
        for (k, v) in &o.extensions {
            let e = self.extensions.entry(k.clone()).or_default();
            e[0] += v[0];
            e[1] += v[1];
        }
        self.max_depth = self.max_depth.max(o.max_depth);
        self.depth_sum += o.depth_sum;
        self.deep += o.deep;
        self.long += o.long;
        self.junk += o.junk;
        for k in 0..4 {
            self.presentation[k] += o.presentation[k];
            self.misplaced[k] += o.misplaced[k];
        }
        self.voice += o.voice;
        self.flash += o.flash;
        self.plugins += o.plugins;
    }
}

// @sat:profile.derive
impl ContentProfile {
    pub fn total(&self) -> u32 {
        self.observed[0] + self.observed[1]
    }

    fn family(&self, f: usize) -> u32 {
        self.families[f][0] + self.families[f][1]
    }

    /// Game content counted for the composition (`modTrackedContentTotal`).
    pub fn tracked(&self) -> u32 {
        (0..RUNTIME_KINDS).map(|f| self.family(f)).sum()
    }

    /// Runtime kinds present, largest first (ties by name).
    pub fn kinds(&self) -> Vec<&'static str> {
        let mut k: Vec<(u32, &str)> = (0..RUNTIME_KINDS)
            .map(|f| (self.family(f), FAMILIES[f].0))
            .filter(|(n, _)| *n > 0)
            .collect();
        k.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(b.1)));
        k.into_iter().map(|(_, n)| n).collect()
    }

    /// The kind with the most files: `mixed` on an exact tie, `none` without game content.
    pub fn primary_kind(&self) -> &'static str {
        let mut best: Vec<(u32, &str)> = (0..RUNTIME_KINDS)
            .map(|f| (self.family(f), FAMILIES[f].0))
            .collect();
        best.sort_by_key(|b| std::cmp::Reverse(b.0));
        match best.as_slice() {
            [(0, _), ..] => "none",
            [(a, _), (b, _), ..] if a == b => "mixed",
            [(_, k), ..] => k,
            [] => "none",
        }
    }

    /// The fewest kinds that make up 80% of the game content (`modContentCoverage80`).
    pub fn coverage80(&self) -> Vec<&'static str> {
        let total = self.tracked();
        let mut acc = 0;
        let mut out = Vec::new();
        for k in self.kinds() {
            if total > 0 && f64::from(acc) >= 0.8 * f64::from(total) {
                break;
            }
            let f = FAMILIES.iter().position(|(n, _)| *n == k).expect("a family");
            acc += self.family(f);
            out.push(k);
        }
        out
    }

    /// Percent of game content inside archives (`modArchivedRuntimeSharePercent`).
    pub fn archived_share(&self) -> f64 {
        let archived = (0..RUNTIME_KINDS).map(|f| self.families[f][ARCHIVED]).sum();
        pct(archived, self.tracked())
    }

    fn cargo_count(&self) -> u32 {
        CARGO.iter().map(|(f, _, _)| self.family(*f)).sum()
    }

    /// Signs of things that are not for the game (installers, sources, build leftovers), strongest first.
    pub fn cargo_signals(&self) -> Vec<&'static str> {
        CARGO
            .iter()
            .filter(|(f, _, _)| self.family(*f) > 0)
            .map(|(_, s, _)| *s)
            .collect()
    }

    /// How sure the cargo verdict is (v604).
    pub fn cargo_confidence(&self) -> f64 {
        let s: Vec<f64> = CARGO
            .iter()
            .filter(|(f, _, _)| self.family(*f) > 0)
            .map(|(_, _, c)| *c)
            .collect();
        match s.iter().copied().reduce(f64::max) {
            Some(max) => (max + 0.05 * (s.len() as f64 - 1.0)).min(0.95),
            None => 0.7,
        }
    }

    /// Composition labels (v605).
    pub fn composition_labels(&self) -> Vec<&'static str> {
        const DOMINANT: [&str; 4] = [
            "texture-dominant",
            "mesh-dominant",
            "script-dominant",
            "animation-dominant",
        ];
        let mut out = Vec::new();
        let total = self.tracked();
        let mut dominant = false;
        if total > 0 {
            for (f, at) in DOMINANT_AT.iter().enumerate() {
                if f64::from(self.family(f)) >= at * f64::from(total) {
                    out.push(DOMINANT[f]);
                    dominant = true;
                }
            }
        }
        if self.family(4) > 0 {
            out.push("native-code-present");
        }
        if total > 0 {
            let share = self.archived_share();
            if share >= 80.0 {
                out.push("archive-backed-runtime");
            } else if share <= 10.0 {
                out.push("loose-runtime");
            }
            if (0..RUNTIME_KINDS).any(|f| self.families[f][LOOSE] > 0)
                && (0..RUNTIME_KINDS).any(|f| self.families[f][ARCHIVED] > 0)
            {
                out.push("mixed-origins");
            }
        }
        let cargo = self.cargo_count();
        if cargo >= 2 && f64::from(cargo) >= 0.25 * f64::from(self.total()) {
            out.push("possible-cargo-heavy");
        }
        if !dominant && self.kinds().len() >= 2 {
            out.push("mixed-content");
        }
        out
    }

    /// Extensions, most files first (ties by name); `""` — files without one.
    pub fn top_extensions(&self, n: usize) -> Vec<(&str, u32)> {
        let mut v: Vec<(&str, u32)> = self
            .extensions
            .iter()
            .map(|(k, c)| (k.as_str(), c[0] + c[1]))
            .collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        v.truncate(n);
        v
    }

    /// Distinct extensions (files without one do not count as a kind).
    pub fn extension_kinds(&self) -> usize {
        self.extensions.keys().filter(|k| !k.is_empty()).count()
    }

    pub fn extensionless(&self) -> u32 {
        self.extensions.get("").map_or(0, |c| c[0] + c[1])
    }

    fn origin_labels(&self, kind: &'static [&'static str; 3]) -> Vec<&'static str> {
        let total = self.total();
        if total == 0 {
            return Vec::new();
        }
        let archived = pct(self.observed[ARCHIVED], total);
        let mut out = Vec::new();
        if archived >= 80.0 {
            out.push(kind[0]);
        } else if archived <= 10.0 {
            out.push(kind[1]);
        }
        if self.observed[LOOSE] > 0 && self.observed[ARCHIVED] > 0 {
            out.push(kind[2]);
        }
        out
    }

    /// Extension distribution labels (v606).
    pub fn extension_labels(&self) -> Vec<&'static str> {
        let total = self.total();
        let mut out = Vec::new();
        if total > 0 {
            let kinds = self.extension_kinds();
            let top = self.top_extensions(5);
            let share = |n: usize| f64::from(top.iter().take(n).map(|t| t.1).sum::<u32>()) / f64::from(total);
            if kinds >= 20 || (kinds >= 12 && share(5) < 0.70) {
                out.push("extension-diverse");
            }
            if kinds >= 3 && share(1) >= 0.70 {
                out.push("extension-concentrated");
            }
            if kinds >= 10 && share(5) < 0.75 {
                out.push("extension-long-tail");
            }
        }
        if self.extensionless() > 0 {
            out.push("extensionless-present");
        }
        out.extend(self.origin_labels(&[
            "mostly-archive-observations",
            "mostly-loose-observations",
            "mixed-file-origins",
        ]));
        out
    }

    fn domain(&self, d: usize) -> u32 {
        self.domains[d][0] + self.domains[d][1]
    }

    /// Path domains present, most files first.
    pub fn domain_names(&self) -> Vec<&'static str> {
        let mut v: Vec<(u32, usize)> = (0..DOMAINS.len())
            .map(|d| (self.domain(d), d))
            .filter(|x| x.0 > 0)
            .collect();
        v.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        v.into_iter().map(|(_, d)| DOMAINS[d]).collect()
    }

    pub fn root_level(&self) -> u32 {
        self.domain(D_ROOT)
    }

    pub fn data_wrapper(&self) -> u32 {
        self.domain(D_DATA)
    }

    /// Path topology labels (v607).
    pub fn topology_labels(&self) -> Vec<&'static str> {
        let total = self.total();
        let mut out = Vec::new();
        let root = self.root_level();
        if root >= 5 && f64::from(root) >= 0.30 * f64::from(total) {
            out.push("root-heavy");
        }
        if total >= 5 {
            let single = (1..DOMAINS.len()).any(|d| f64::from(self.domain(d)) >= 0.90 * f64::from(total));
            if single {
                out.push("single-top-level-domain");
            }
        }
        if (0..DOMAINS.len()).filter(|d| self.domain(*d) > 0).count() >= 6 {
            out.push("broad-path-topology");
        }
        if self.deep > 0 {
            out.push("deep-paths");
        }
        if self.long > 0 {
            out.push("long-paths");
        }
        if self.data_wrapper() > 0 {
            out.push("data-wrapper");
        }
        if self.junk > 0 {
            out.push("system-junk");
        }
        out.extend(self.origin_labels(&["mostly-archive-paths", "mostly-loose-paths", "mixed-path-origins"]));
        out
    }

    // @sat:profile.package
    /// What kind of package this is (v608): `empty`, `plugin-and-assets`, `plugin-centric`, `asset-only`,
    /// `support-only` or `other`.
    pub fn package_shape(&self) -> &'static str {
        let runtime = RUNTIME_DOMAINS.clone().any(|d| self.domain(d) > 0) || self.family(5) > 0;
        let support = SUPPORT_DOMAINS.clone().any(|d| self.domain(d) > 0);
        match (self.plugins > 0, runtime) {
            _ if self.total() == 0 && self.plugins == 0 => "empty",
            (true, true) => "plugin-and-assets",
            (true, false) => "plugin-centric",
            (false, true) => "asset-only",
            (false, false) if support => "support-only",
            _ => "other",
        }
    }

    /// Presentation kinds present (v609).
    pub fn presentation_kinds(&self) -> Vec<&'static str> {
        (0..4)
            .filter(|k| self.presentation[*k] > 0)
            .map(|k| PRESENTATION[k].0)
            .collect()
    }

    /// Presentation labels (v609).
    pub fn presentation_labels(&self) -> Vec<&'static str> {
        const MISPLACED: [&str; 4] = [
            "localization-path-format-mismatch",
            "interface-path-format-mismatch",
            "audio-path-format-mismatch",
            "video-path-format-mismatch",
        ];
        let mut out = Vec::new();
        if self.voice > 0 {
            out.push("voice-package-likely");
        }
        if self.flash > 0 {
            out.push("flash-interface-package");
        }
        out.extend((0..4).filter(|k| self.misplaced[*k] > 0).map(|k| MISPLACED[k]));
        out
    }

    /// Every label, in the order of the profiles.
    pub fn labels(&self) -> Vec<&'static str> {
        let mut out = self.composition_labels();
        out.extend(self.extension_labels());
        out.extend(self.topology_labels());
        out.extend(self.presentation_labels());
        if !self.cargo_signals().is_empty() {
            out.push("possible-non-runtime-cargo");
        }
        out.sort_unstable();
        out.dedup();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loose(p: &mut ContentProfile, path: &str) {
        let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
        p.observe(DirShape::of(dir), name, LOOSE);
    }

    #[test]
    fn a_texture_mod_with_an_archive_and_cargo() {
        let mut p = ContentProfile {
            plugins: 1,
            ..Default::default()
        };
        for i in 0..7 {
            loose(&mut p, &format!("Textures/Armor/a{i}.dds"));
        }
        loose(&mut p, "meshes/armor/a.nif");
        loose(&mut p, "Mod.bsa");
        loose(&mut p, "fomod/ModuleConfig.xml");
        loose(&mut p, "source/a.psd");
        loose(&mut p, "Optional.7z");
        loose(&mut p, "readme");
        p.observe_path("textures/armor/b.dds", ARCHIVED);
        p.observe_path("textures/armor/c.dds", ARCHIVED);

        assert_eq!(p.total(), 15);
        assert_eq!(p.tracked(), 10);
        assert_eq!(p.primary_kind(), "textures");
        assert_eq!(p.kinds(), ["textures", "meshes"]);
        assert_eq!(p.coverage80(), ["textures"]);
        assert_eq!(p.archived_share(), 20.0);
        assert_eq!(p.cargo_signals(), ["nested-archives", "authoring-sources"]);
        assert!((p.cargo_confidence() - 0.90).abs() < 1e-9);
        let c = p.composition_labels();
        assert!(
            c.contains(&"texture-dominant") && c.contains(&"mixed-origins"),
            "{c:?}"
        );
        assert!(!c.contains(&"mixed-content"));
        assert_eq!(p.top_extensions(2), [("dds", 9), ("", 1)]);
        assert_eq!(p.extensionless(), 1);
        assert_eq!(p.package_shape(), "plugin-and-assets");
        assert_eq!(p.domain_names()[0], "textures");
        assert!(p.labels().contains(&"possible-non-runtime-cargo"));
        assert!(p.extension_labels().contains(&"extensionless-present"));
    }

    #[test]
    fn paths_voice_and_shapes() {
        let mut p = ContentProfile::default();
        assert_eq!(p.package_shape(), "empty");
        assert_eq!(p.primary_kind(), "none");
        loose(&mut p, "Data/Sound/Voice/x.esp/npc/line.fuz");
        loose(&mut p, "sound/voice/x.esp/npc/line.fuz");
        loose(&mut p, "__MACOSX/._a.dds");
        loose(&mut p, "a/b/c/d/e/f/g/h.txt");
        loose(&mut p, &format!("docs/{}.txt", "x".repeat(200)));
        loose(&mut p, "interface/hud.swf");
        loose(&mut p, "readme.html");
        let t = p.topology_labels();
        for l in [
            "data-wrapper",
            "system-junk",
            "deep-paths",
            "long-paths",
            "mostly-loose-paths",
        ] {
            assert!(t.contains(&l), "{l} in {t:?}");
        }
        let pr = p.presentation_labels();
        assert_eq!(
            pr,
            [
                "voice-package-likely",
                "flash-interface-package",
                "interface-path-format-mismatch",
                "audio-path-format-mismatch"
            ]
        );
        assert_eq!(p.max_depth, 8);
        assert_eq!(p.package_shape(), "asset-only");

        let mut docs = ContentProfile::default();
        loose(&mut docs, "docs/readme.txt");
        assert_eq!(docs.package_shape(), "support-only");
        let mut esp = ContentProfile {
            plugins: 1,
            ..Default::default()
        };
        loose(&mut esp, "x.esp");
        assert_eq!(esp.package_shape(), "plugin-centric");

        let mut tie = ContentProfile::default();
        loose(&mut tie, "a.dds");
        loose(&mut tie, "a.nif");
        assert_eq!(tie.primary_kind(), "mixed");
        // half meshes reaches the mesh threshold (0.50), half textures does not (0.60)
        assert_eq!(tie.composition_labels(), ["mesh-dominant", "loose-runtime"]);
        let mut g = p.clone();
        g.merge(&tie);
        assert_eq!(g.total(), p.total() + 2);
    }
}
