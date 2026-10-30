//! Facts: what scanners observed about one mod folder. Input to the warm tier.
//!
//! These types are produced only by scanners (`sat-scan`) and are serializable for the cold-tier scan cache.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use sat_formats::{ArchiveManifest, PluginHeader, RecordCounts};

/// One loose file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LooseFile {
    pub name: String,
    pub size: u64,
    /// Exact GPU bytes for DDS when the header was read; 0 = unknown / not a texture.
    pub vram: u64,
}

/// Files of one directory, `rel` uses `/` and is `""` for the mod root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LooseDir {
    pub rel: String,
    pub files: Vec<LooseFile>,
}

/// A BSA/BA2 found at the mod root.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArchiveFacts {
    pub file_name: String,
    pub size: u64,
    pub manifest: Option<ArchiveManifest>,
    pub error: Option<String>,
}

/// A plugin found at the mod root.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginFacts {
    pub file_name: String,
    pub header: Option<PluginHeader>,
    pub records: Option<RecordCounts>,
    pub error: Option<String>,
}

/// Cheap aggregates computed during the walk (no second traversal).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModMetrics {
    pub bytes: u64,
    pub files: u32,
    pub dirs: u32,
    pub root_files: u32,
    pub max_depth: u32,
    pub dds: u32,
    pub dds_known: u32,
    pub vram_bytes: u64,
    pub nif: u32,
    pub pex: u32,
    pub hkx: u32,
    pub dll: u32,
    pub plugins: u32,
    pub archives: u32,
    pub archive_files: u32,
    /// zip/7z/rar shipped inside the mod — likely installer cargo.
    pub nested_archives: u32,
    /// psd/xcf/blend/psc… — authoring sources, not runtime content.
    pub source_files: u32,
    pub by_extension: BTreeMap<String, u32>,
    pub by_top_dir: BTreeMap<String, u32>,
    pub dds_formats: BTreeMap<String, u32>,
}

/// Everything known about one mod folder after a scan.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ModFacts {
    /// Folder name (= MO2 mod name).
    pub name: String,
    pub dirs: Vec<LooseDir>,
    pub archives: Vec<ArchiveFacts>,
    pub plugins: Vec<PluginFacts>,
    pub metrics: ModMetrics,
    /// Folder does not exist in any mod root.
    pub missing: bool,
    /// Non-fatal problem while walking (unreadable subfolder, …).
    pub error: Option<String>,
}

impl ModFacts {
    pub fn missing(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            missing: true,
            ..Default::default()
        }
    }

    /// Number of providers this mod contributes to the table (loose files + archive entries).
    pub fn provider_count(&self) -> usize {
        self.dirs.iter().map(|d| d.files.len()).sum::<usize>()
            + self
                .archives
                .iter()
                .filter_map(|a| a.manifest.as_ref())
                .map(|m| m.entries.len())
                .sum::<usize>()
    }
}
