//! Integration test: scanning a real folder. @sat:scan.it
//!
//! Owns: integration test of a real folder scan with DDS, plugin, archive and cache.

use std::fs;
use std::path::Path;

use sat_scan::{ScanJob, ScanOptions, ScanProgress, scan};

fn dds(w: u32, h: u32) -> Vec<u8> {
    let mut b = vec![0u8; 128];
    b[0..4].copy_from_slice(b"DDS ");
    b[12..16].copy_from_slice(&h.to_le_bytes());
    b[16..20].copy_from_slice(&w.to_le_bytes());
    b[28..32].copy_from_slice(&1u32.to_le_bytes());
    b[80..84].copy_from_slice(&4u32.to_le_bytes());
    b[84..88].copy_from_slice(b"DXT1");
    b
}

fn plugin() -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(b"HEDR");
    body.extend_from_slice(&12u16.to_le_bytes());
    body.extend_from_slice(&[0; 12]);
    let mut out = b"TES4".to_vec();
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&[0; 16]);
    out.extend(body);
    out
}

fn write(p: &Path, data: &[u8]) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, data).unwrap();
}

#[test]
fn scans_mod_with_dds_plugin_archive_and_uses_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let m = tmp.path().join("Mod A");
    write(&m.join("meta.ini"), b"[General]\n");
    write(&m.join("ModA.esp"), &plugin());
    write(
        &m.join("ModA.bsa"),
        &sat_formats::archive::fixtures::bsa_v105(&[("textures\\x", &["a.dds"])]),
    );
    write(&m.join("textures/x/b.dds"), &dds(256, 256));
    write(&m.join("textures/y/c.dds"), &dds(64, 64));
    write(&m.join("meshes/m.nif"), b"nif");
    write(&m.join("src/tex.psd"), b"psd");

    let cache = tmp.path().join("cache");
    let opts = ScanOptions {
        cache_dir: Some(cache.clone()),
        ..Default::default()
    };
    let jobs = vec![
        ScanJob {
            name: "Mod A".into(),
            path: m.clone(),
        },
        ScanJob {
            name: "Ghost".into(),
            path: tmp.path().join("nope"),
        },
    ];
    let progress = ScanProgress::default();
    let out = scan(&jobs, &opts, &progress);
    assert!(out[1].facts.missing);
    let f = &out[0].facts;
    assert!(!out[0].from_cache);
    assert_eq!(f.metrics.files, 6, "meta.ini is ignored");
    assert_eq!(f.metrics.dds, 2);
    assert_eq!(f.metrics.dds_known, 2);
    assert_eq!(f.metrics.vram_bytes, 256 * 256 / 2 + 64 * 64 / 2);
    assert_eq!(f.metrics.plugins, 1);
    assert_eq!(f.metrics.archives, 1);
    assert_eq!(f.metrics.archive_files, 1);
    assert_eq!(f.metrics.source_files, 1);
    assert_eq!(f.plugins[0].header.as_ref().unwrap().num_records, 0);
    assert_eq!(
        f.archives[0].manifest.as_ref().unwrap().entries[0].path,
        "textures/x/a.dds"
    );
    let rels: Vec<_> = f.dirs.iter().map(|d| d.rel.as_str()).collect();
    assert_eq!(
        rels,
        ["", "meshes", "src", "textures/x", "textures/y"],
        "deterministic order"
    );
    assert_eq!(f.provider_count(), 7);

    // second run: from cache, identical facts
    let again = scan(&jobs[..1], &opts, &ScanProgress::default());
    assert!(again[0].from_cache);
    assert_eq!(&again[0].facts, f);

    // touching a top-level dir invalidates the signature
    write(&m.join("meshes/new.nif"), b"x");
    let third = scan(&jobs[..1], &opts, &ScanProgress::default());
    assert!(!third[0].from_cache);
    assert_eq!(third[0].facts.metrics.nif, 2);
}
