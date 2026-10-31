//! Mod Organizer 2 adapter. @sat:mo2
//!
//! - [`ini`] — Qt-style INI reading, `@ByteArray(...)`, `@Variant(...)` QColor;
//! - [`instance`] — resolving an instance (`ModOrganizer.ini`, `%BASE_DIR%`, profiles, extra mod roots);
//! - [`profile`] — `modlist.txt` ⇄ [`sat_core::canon::Modlist`] (byte-exact round trip), `plugins.txt`, `loadorder.txt`;
//! - [`meta`] — a mod's `meta.ini`.
//!
//! This crate is the only place that knows MO2 file formats (ADR 0002: canon is written only by its adapter).
//!
//! Owns: MO2 adapter layout, errors, atomic file writes.

pub mod ini;
pub mod instance;
pub mod meta;
pub mod profile;

pub use instance::Instance;
pub use meta::ModMeta;

#[derive(Debug, thiserror::Error)]
pub enum Mo2Error {
    #[error("{path}: {source}")]
    Io {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    #[error("not an MO2 instance: {0}")]
    NotInstance(String),
}

pub type Result<T> = std::result::Result<T, Mo2Error>;

pub(crate) fn read(path: &std::path::Path) -> Result<Vec<u8>> {
    std::fs::read(path).map_err(|source| Mo2Error::Io {
        path: path.to_path_buf(),
        source,
    })
}

// @sat:mo2.write-atomic
/// Writes through a temp file and rename, so a crash never leaves a half-written profile file.
pub fn write_atomic(path: &std::path::Path, data: &[u8]) -> Result<()> {
    let io = |source| Mo2Error::Io {
        path: path.to_path_buf(),
        source,
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(io)?;
    }
    let tmp = path.with_extension("sat-tmp");
    std::fs::write(&tmp, data).map_err(io)?;
    std::fs::rename(&tmp, path).map_err(io)
}
