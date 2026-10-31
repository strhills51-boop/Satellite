//! Analyzer contract (NG-D002 p.1a, monolith Analysis Orchestrator v503 CP2–CP15).
//!
//! The walker turns a mod folder into **observations** (loose files; archive entries as virtual files) and hands
//! each one, in registration order, to every analyzer that accepts it. Built-in analyzers are registered exactly
//! the way extensions will be.
//!
//! Every analyzer has a version and a config hash; the cache stores the stamps it was computed with. When only
//! some stamps are stale, the mod is **re-analyzed from its cached path index** (route `indexed`) by the stale
//! analyzers alone — no directory walk. A stale *structural* analyzer (the one that produces the path index)
//! forces a full walk (route `scan`). This replaces the monolith's single `artifactCapabilities` set and NG's
//! former single `CACHE_VERSION`.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use sat_core::facts::{ArchiveFacts, LooseFile, ModFacts, ModMetrics, PluginFacts};
use sat_formats::{DdsInfo, dds::DDS_HEADER_PROBE};

use crate::signature::Fnv;
use crate::{ScanOptions, ScanProgress, add_ns};

/// How an analyzer takes part in the walk (monolith `EXECUTION_CLASS`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExecutionClass {
    /// Reads observations, writes facts.
    StreamConsumer,
    /// Opens a container and produces virtual observations (archive entries) for other analyzers.
    SourceExpander,
}

/// What a failure means (monolith `FAILURE_POLICY`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FailurePolicy {
    /// The mod's facts are incomplete: recorded as the mod's error, the result is not cached.
    Mandatory,
    /// The analyzer's own facts are missing; the rest of the mod is fine.
    Optional,
    /// Facts are partial (e.g. one unreadable texture); reported, the run goes on.
    Degraded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Contract {
    /// `[a-z0-9][a-z0-9._:-]*` (monolith `CONTRACT_ID_RE`).
    pub id: &'static str,
    /// Bump when the facts it produces change meaning: stale mods are re-analyzed.
    pub version: u32,
    pub class: ExecutionClass,
    pub failure: FailurePolicy,
    /// Produces the path index itself; when stale, the folder is walked again.
    pub structural: bool,
    /// Consumes virtual observations (archive entries); re-runs whenever an expander re-runs.
    pub reads_virtual: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Loose,
    /// An entry of an archive at the mod root.
    Archive,
}

/// One thing seen in a mod folder.
#[derive(Debug, Clone, Copy)]
pub struct Observation<'a> {
    pub origin: Origin,
    /// Loose: directory relative to the mod root (`""` = root). Archive: the archive's file name.
    pub dir: &'a str,
    /// Loose: file name. Archive: entry path inside the archive.
    pub name: &'a str,
    /// Lowercase extension without the dot.
    pub ext: &'a str,
    pub size: u64,
    /// Loose: 0 = mod root. Archive: 0.
    pub depth: u32,
    /// Absolute path of a loose file.
    pub path: Option<&'a Path>,
    /// Archive entry: GPU bytes known from the archive index (BA2 DX10); 0 otherwise.
    pub vram: u64,
}

/// Where an analyzer writes.
#[allow(missing_debug_implementations)]
pub struct Part<'a> {
    pub metrics: &'a mut ModMetrics,
    /// The loose file being observed.
    pub file: Option<&'a mut LooseFile>,
    pub plugins: &'a mut Vec<PluginFacts>,
    pub archives: &'a mut Vec<ArchiveFacts>,
}

pub trait Analyzer: Send + Sync {
    fn contract(&self) -> Contract;
    /// Settings that change the result (part of the cache stamp).
    fn config(&self) -> u64 {
        0
    }
    fn accepts(&self, o: &Observation<'_>) -> bool;
    fn consume(
        &self,
        o: &Observation<'_>,
        part: &mut Part<'_>,
        progress: &ScanProgress,
    ) -> Result<(), String>;
    /// Removes this analyzer's facts before it re-runs on a cached index.
    fn clear(&self, facts: &mut ModFacts);
}

/// What a cached result was computed with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stamp {
    pub id: String,
    pub version: u32,
    pub config: u64,
}

/// Analyzers in dispatch order.
#[allow(missing_debug_implementations)]
pub struct Registry {
    list: Vec<Box<dyn Analyzer>>,
    /// Known but disabled: kept to clear their facts from cached results.
    dormant: Vec<Box<dyn Analyzer>>,
}

fn valid_id(id: &str) -> bool {
    let b = id.as_bytes();
    !b.is_empty()
        && b[0].is_ascii_alphanumeric()
        && b.iter().all(|c| c.is_ascii_alphanumeric() || b"._:-".contains(c))
}

impl Registry {
    pub fn empty() -> Self {
        Self {
            list: Vec::new(),
            dormant: Vec::new(),
        }
    }

    /// Built-in analyzers enabled by the options: `metrics` always, then `dds-vram`, `plugins`, `archives`.
    pub fn builtin(opts: &ScanOptions) -> Self {
        let mut r = Self::empty();
        let all: [(bool, Box<dyn Analyzer>); 4] = [
            (
                true,
                Box::new(Metrics {
                    ignore_root: opts.ignore_root_files.clone(),
                }),
            ),
            (opts.read_dds_headers, Box::new(DdsVram)),
            (
                opts.plugins,
                Box::new(Plugins {
                    records: opts.record_scan,
                }),
            ),
            (opts.archives, Box::new(Archives)),
        ];
        for (on, a) in all {
            if on {
                r.register(a).expect("built-in contracts are valid");
            } else {
                r.dormant.push(a);
            }
        }
        r
    }

    /// Adds an analyzer at the end of the dispatch order.
    pub fn register(&mut self, a: Box<dyn Analyzer>) -> Result<(), String> {
        let c = a.contract();
        if !valid_id(c.id) {
            return Err(format!("invalid analyzer id {:?}", c.id));
        }
        if self.list.iter().any(|x| x.contract().id == c.id) {
            return Err(format!("analyzer {:?} registered twice", c.id));
        }
        if c.structural && c.class == ExecutionClass::SourceExpander {
            return Err(format!("{}: an expander cannot be structural", c.id));
        }
        self.list.push(a);
        Ok(())
    }

    pub fn stamps(&self) -> Vec<Stamp> {
        self.list
            .iter()
            .map(|a| {
                let c = a.contract();
                Stamp {
                    id: c.id.to_string(),
                    version: c.version,
                    config: a.config(),
                }
            })
            .collect()
    }

    pub fn contracts(&self) -> Vec<Contract> {
        self.list.iter().map(|a| a.contract()).collect()
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &dyn Analyzer> {
        self.list.iter().map(|a| a.as_ref())
    }

    /// An enabled or dormant analyzer.
    pub(crate) fn get(&self, id: &str) -> Option<&dyn Analyzer> {
        self.list
            .iter()
            .chain(&self.dormant)
            .map(|a| a.as_ref())
            .find(|a| a.contract().id == id)
    }
}

/// How a mod's facts were obtained this run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "route", rename_all = "kebab-case")]
pub enum Route {
    /// Walked from disk.
    Scan,
    /// Cached index: facts of `cleared` (disabled analyzers) removed, `rerun` analyzers run again.
    Indexed {
        rerun: Vec<String>,
        cleared: Vec<String>,
    },
    /// Cached facts as they are.
    Reuse,
}

/// Decides the route from the stamps a cached result was computed with.
pub(crate) fn plan(reg: &Registry, cached: &[Stamp]) -> Route {
    let now = reg.stamps();
    if now == cached {
        return Route::Reuse;
    }
    let mut rerun: Vec<String> = Vec::new();
    let mut expander = false;
    for (a, s) in reg.iter().zip(&now) {
        if !cached.contains(s) {
            if a.contract().structural {
                return Route::Scan;
            }
            expander |= a.contract().class == ExecutionClass::SourceExpander;
            rerun.push(s.id.clone());
        }
    }
    // A disabled analyzer leaves facts behind: a known one clears them, an unknown one forces a walk.
    let mut cleared = Vec::new();
    for s in cached {
        if !now.iter().any(|n| n.id == s.id) {
            match reg.get(&s.id) {
                Some(a) if !a.contract().structural => {
                    expander |= a.contract().class == ExecutionClass::SourceExpander;
                    cleared.push(s.id.clone());
                }
                _ => return Route::Scan,
            }
        }
    }
    if expander {
        for a in reg.iter() {
            let c = a.contract();
            if c.reads_virtual && !rerun.iter().any(|r| r == c.id) {
                rerun.push(c.id.to_string());
            }
        }
    }
    Route::Indexed { rerun, cleared }
}

/// Hands a loose observation to the accepting analyzers of `only` (all when `None`).
pub(crate) fn dispatch(
    reg: &Registry,
    only: Option<&[String]>,
    o: &Observation<'_>,
    part: &mut Part<'_>,
    progress: &ScanProgress,
    issues: &mut Issues,
) {
    for a in reg.iter() {
        let c = a.contract();
        if only.is_some_and(|ids| !ids.iter().any(|i| i == c.id)) || !a.accepts(o) {
            continue;
        }
        if let Err(e) = a.consume(o, part, progress) {
            issues.push(
                c,
                format!(
                    "{}{}{}: {e}",
                    o.dir,
                    if o.dir.is_empty() { "" } else { "/" },
                    o.name
                ),
            );
        }
    }
}

/// Virtual observations of every archive manifest, to analyzers that read them.
pub(crate) fn dispatch_virtual(
    reg: &Registry,
    only: Option<&[String]>,
    facts: &mut ModFacts,
    progress: &ScanProgress,
    issues: &mut Issues,
) {
    let archives = std::mem::take(&mut facts.archives);
    let mut plugins = std::mem::take(&mut facts.plugins);
    let mut none = Vec::new();
    for a in &archives {
        let Some(m) = &a.manifest else { continue };
        for e in &m.entries {
            let ext = e.path.rsplit_once('.').map_or("", |(_, x)| x);
            let o = Observation {
                origin: Origin::Archive,
                dir: &a.file_name,
                name: &e.path,
                ext,
                size: e.size,
                depth: 0,
                path: None,
                vram: e.vram,
            };
            let mut part = Part {
                metrics: &mut facts.metrics,
                file: None,
                plugins: &mut plugins,
                archives: &mut none,
            };
            for an in reg.iter() {
                let c = an.contract();
                if !c.reads_virtual
                    || c.class == ExecutionClass::SourceExpander
                    || only.is_some_and(|ids| !ids.iter().any(|i| i == c.id))
                    || !an.accepts(&o)
                {
                    continue;
                }
                if let Err(err) = an.consume(&o, &mut part, progress) {
                    issues.push(c, format!("{}:{}: {err}", a.file_name, e.path));
                }
            }
        }
    }
    facts.archives = archives;
    facts.plugins = plugins;
}

/// Problems reported by analyzers during one mod.
#[derive(Debug, Default)]
pub struct Issues {
    /// A mandatory analyzer failed: facts are incomplete.
    pub fatal: Option<String>,
    /// Optional and degraded failures (`analyzer: message`).
    pub other: Vec<String>,
}

impl Issues {
    fn push(&mut self, c: Contract, msg: String) {
        let line = format!("{}: {msg}", c.id);
        match c.failure {
            FailurePolicy::Mandatory => {
                if self.fatal.is_none() {
                    self.fatal = Some(line);
                }
            }
            _ => self.other.push(line),
        }
    }
    pub(crate) fn merge(&mut self, b: Issues) {
        if self.fatal.is_none() {
            self.fatal = b.fatal;
        }
        self.other.extend(b.other);
    }
}

// ---------------------------------------------------------------- built-in analyzers

const NESTED: &[&str] = &["zip", "7z", "rar", "tar", "gz"];
const SOURCES: &[&str] = &["psc", "psd", "xcf", "blend", "max", "fbx", "obj", "spp", "sbsar"];

/// Counts and sizes of the path index (structural: its facts are the index).
struct Metrics {
    ignore_root: Vec<String>,
}

impl Analyzer for Metrics {
    fn contract(&self) -> Contract {
        Contract {
            id: "metrics",
            version: 1,
            class: ExecutionClass::StreamConsumer,
            failure: FailurePolicy::Mandatory,
            structural: true,
            reads_virtual: false,
        }
    }
    fn config(&self) -> u64 {
        let mut h = Fnv::new();
        for n in &self.ignore_root {
            h.write(n.as_bytes());
            h.write(b"|");
        }
        h.finish()
    }
    fn accepts(&self, o: &Observation<'_>) -> bool {
        o.origin == Origin::Loose
    }
    fn consume(&self, o: &Observation<'_>, part: &mut Part<'_>, _: &ScanProgress) -> Result<(), String> {
        let m = &mut *part.metrics;
        m.files += 1;
        m.bytes += o.size;
        *m.by_extension.entry(o.ext.to_string()).or_insert(0) += 1;
        if o.depth == 0 {
            m.root_files += 1;
            match o.ext {
                "esp" | "esm" | "esl" => m.plugins += 1,
                "bsa" | "ba2" => m.archives += 1,
                _ => {}
            }
        } else {
            let top = o.dir.split('/').next().unwrap_or(o.dir).to_lowercase();
            *m.by_top_dir.entry(top).or_insert(0) += 1;
        }
        m.max_depth = m.max_depth.max(o.depth);
        match o.ext {
            "dds" => m.dds += 1,
            "nif" => m.nif += 1,
            "pex" => m.pex += 1,
            "hkx" => m.hkx += 1,
            "dll" => m.dll += 1,
            x if NESTED.contains(&x) => m.nested_archives += 1,
            x if SOURCES.contains(&x) => m.source_files += 1,
            _ => {}
        }
        Ok(())
    }
    fn clear(&self, _: &mut ModFacts) {
        unreachable!("structural analyzers re-run by walking")
    }
}

/// GPU memory of textures: loose DDS headers and archive entries with a known size (monolith CP15).
struct DdsVram;

impl Analyzer for DdsVram {
    fn contract(&self) -> Contract {
        Contract {
            id: "dds-vram",
            version: 1,
            class: ExecutionClass::StreamConsumer,
            failure: FailurePolicy::Degraded,
            structural: false,
            reads_virtual: true,
        }
    }
    fn accepts(&self, o: &Observation<'_>) -> bool {
        match o.origin {
            Origin::Loose => o.ext == "dds",
            Origin::Archive => o.vram > 0,
        }
    }
    fn consume(
        &self,
        o: &Observation<'_>,
        part: &mut Part<'_>,
        progress: &ScanProgress,
    ) -> Result<(), String> {
        let m = &mut *part.metrics;
        if o.origin == Origin::Archive {
            m.vram_bytes += o.vram;
            m.dds_known += 1;
            return Ok(());
        }
        let t = Instant::now();
        let info = o.path.and_then(read_dds);
        add_ns(&progress.dds_ns, t);
        progress.dds_reads.fetch_add(1, Ordering::Relaxed);
        if let Some(info) = info
            && info.known()
        {
            if let Some(f) = part.file.as_deref_mut() {
                f.vram = info.vram;
            }
            m.dds_known += 1;
            m.vram_bytes += info.vram;
            *m.dds_formats.entry(info.format).or_insert(0) += 1;
        }
        // An unknown format is not a failure: the texture counts as `dds` without `dds_known`.
        Ok(())
    }
    fn clear(&self, facts: &mut ModFacts) {
        let m = &mut facts.metrics;
        m.dds_known = 0;
        m.vram_bytes = 0;
        m.dds_formats.clear();
        for f in facts.dirs.iter_mut().flat_map(|d| d.files.iter_mut()) {
            f.vram = 0;
        }
    }
}

fn read_dds(path: &Path) -> Option<DdsInfo> {
    let mut f = File::open(path).ok()?;
    let mut buf = [0u8; DDS_HEADER_PROBE];
    let mut n = 0;
    while n < buf.len() {
        match f.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(_) => return None,
        }
    }
    DdsInfo::parse(&buf[..n])
}

/// Root plugins: TES4 header, optionally record counts (monolith CP9 + v611).
struct Plugins {
    records: bool,
}

impl Analyzer for Plugins {
    fn contract(&self) -> Contract {
        Contract {
            id: "plugins",
            version: 1,
            class: ExecutionClass::StreamConsumer,
            failure: FailurePolicy::Optional,
            structural: false,
            reads_virtual: false,
        }
    }
    fn config(&self) -> u64 {
        u64::from(self.records)
    }
    fn accepts(&self, o: &Observation<'_>) -> bool {
        o.origin == Origin::Loose && o.depth == 0 && matches!(o.ext, "esp" | "esm" | "esl")
    }
    fn consume(
        &self,
        o: &Observation<'_>,
        part: &mut Part<'_>,
        progress: &ScanProgress,
    ) -> Result<(), String> {
        let t = Instant::now();
        let p = read_plugin(o.path.ok_or("no path")?, o.name, self.records);
        add_ns(&progress.plugin_ns, t);
        progress.plugin_reads.fetch_add(1, Ordering::Relaxed);
        let err = p.error.clone();
        part.plugins.push(p);
        err.map_or(Ok(()), Err)
    }
    fn clear(&self, facts: &mut ModFacts) {
        facts.plugins.clear();
    }
}

fn read_plugin(path: &Path, name: &str, records: bool) -> PluginFacts {
    let mut p = PluginFacts {
        file_name: name.to_string(),
        header: None,
        records: None,
        error: None,
    };
    match File::open(path)
        .map_err(sat_formats::FormatError::from)
        .and_then(|f| sat_formats::read_plugin_header(BufReader::new(f)))
    {
        Ok(h) => p.header = Some(h),
        Err(e) => {
            p.error = Some(e.to_string());
            return p;
        }
    }
    if records {
        match File::open(path)
            .map_err(sat_formats::FormatError::from)
            .and_then(|f| sat_formats::scan_records(BufReader::with_capacity(1 << 20, f)))
        {
            Ok(r) => p.records = Some(r),
            Err(e) => p.error = Some(e.to_string()),
        }
    }
    p
}

/// Root BSA/BA2: index of entries, which become virtual observations (monolith CP6/CP7 source expander).
struct Archives;

impl Analyzer for Archives {
    fn contract(&self) -> Contract {
        Contract {
            id: "archives",
            version: 1,
            class: ExecutionClass::SourceExpander,
            failure: FailurePolicy::Degraded,
            structural: false,
            reads_virtual: false,
        }
    }
    fn accepts(&self, o: &Observation<'_>) -> bool {
        o.origin == Origin::Loose && o.depth == 0 && matches!(o.ext, "bsa" | "ba2")
    }
    fn consume(
        &self,
        o: &Observation<'_>,
        part: &mut Part<'_>,
        progress: &ScanProgress,
    ) -> Result<(), String> {
        let t = Instant::now();
        let a = read_archive(o.path.ok_or("no path")?, o.name, o.size);
        add_ns(&progress.archive_ns, t);
        progress.archive_reads.fetch_add(1, Ordering::Relaxed);
        if let Some(m) = &a.manifest {
            part.metrics.archive_files += m.entries.len() as u32;
        }
        let err = a.error.clone();
        part.archives.push(a);
        err.map_or(Ok(()), Err)
    }
    fn clear(&self, facts: &mut ModFacts) {
        facts.archives.clear();
        facts.metrics.archive_files = 0;
    }
}

fn read_archive(path: &Path, name: &str, size: u64) -> ArchiveFacts {
    match File::open(path)
        .map_err(sat_formats::FormatError::from)
        .and_then(sat_formats::read_archive)
    {
        Ok(m) => ArchiveFacts {
            file_name: name.to_string(),
            size,
            manifest: Some(m),
            error: None,
        },
        Err(e) => ArchiveFacts {
            file_name: name.to_string(),
            size,
            manifest: None,
            error: Some(e.to_string()),
        },
    }
}
