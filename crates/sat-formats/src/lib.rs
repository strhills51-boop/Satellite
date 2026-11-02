//! Parsers for Bethesda game data formats. @sat:formats
//!
//! Every parser works on `Read`/`Read + Seek` sources and never touches the filesystem,
//! so the crate compiles to WebAssembly unchanged (ADR 0001, ADR 0002).
//!
//! Owns: the format crate layout and path normalization of archive entries.

pub mod archive;
pub mod dds;
pub mod fomod;
pub mod ini;
pub mod plugin;
mod read;

pub use archive::{ArchiveEntry, ArchiveKind, ArchiveManifest, read_archive};
pub use dds::{DdsInfo, dxgi_name, texture_bytes};
pub use plugin::{
    PluginHeader, PluginKind, RecordCounts, monolith_plugin_type, read_plugin_header, scan_records,
};

/// Errors produced by the parsers.
#[derive(Debug, thiserror::Error)]
pub enum FormatError {
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("unexpected signature {found:?}, expected {expected}")]
    Signature { found: [u8; 4], expected: &'static str },
    #[error("unsupported version {0}")]
    Version(u32),
    #[error("corrupt data: {0}")]
    Corrupt(&'static str),
}

pub type Result<T> = std::result::Result<T, FormatError>;

/// Normalizes an in-game path: forward slashes, lowercase, no leading/trailing slash.
pub fn normalize_path(p: &str) -> String {
    let s: String = p
        .chars()
        .map(|c| if c == '\\' { '/' } else { c.to_ascii_lowercase() })
        .collect();
    s.trim_matches('/').to_string()
}
