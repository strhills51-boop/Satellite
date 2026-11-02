//! Times saving a project file with many mods' facts. @sat:stll.bench
//!
//! Owns: the save/open/read timing of `project.stll` at build scale (`cargo run --release -p sat-stll --example
//! save_bench [MODS] [FILES]`; 7 500 × 400: first save 0.5 s, 3 MiB, unchanged save 0.4 s).
//! Not here: the file (`@sat:stll.file`).
use sat_core::canon::{Entry, EntryKind, Modlist};
use sat_core::facts::{LooseDir, LooseFile, ModFacts};
use sat_core::project::Project;
use std::time::Instant;

fn main() {
    let args: Vec<usize> = std::env::args().skip(1).filter_map(|a| a.parse().ok()).collect();
    let (mods, files) = (
        args.first().copied().unwrap_or(7500),
        args.get(1).copied().unwrap_or(400),
    );
    let names: Vec<String> = (0..mods).map(|i| format!("Mod number {i:05}")).collect();
    let p = Project::import(Modlist::new(
        names
            .iter()
            .map(|n| Entry {
                name: n.clone(),
                kind: EntryKind::Mod,
                enabled: true,
            })
            .collect(),
    ));
    let facts = |i: usize| ModFacts {
        name: names[i].clone(),
        dirs: (0..files / 20)
            .map(|d| LooseDir {
                rel: format!("textures/mod{i}/set{d}"),
                files: (0..20)
                    .map(|f| LooseFile {
                        name: format!("texture_{f:03}_d.dds"),
                        size: 1 << 20,
                        vram: 1 << 20,
                    })
                    .collect(),
            })
            .collect(),
        ..ModFacts::default()
    };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("project.stll");
    let mut f = sat_stll::Stll::new(&path);
    let t = Instant::now();
    let mut w = f.begin().unwrap();
    for (i, e) in p.entities.iter().enumerate() {
        w.put_facts(&e.id.0, i as u64, &facts(i)).unwrap();
    }
    w.finish(&p, sat_stll::file::keep_entities(&p)).unwrap();
    let first = t.elapsed();
    let size = std::fs::metadata(&path).unwrap().len();
    let t = Instant::now();
    let (mut g, _) = sat_stll::Stll::open(&path).unwrap();
    let open = t.elapsed();
    let t = Instant::now();
    let mut w = g.begin().unwrap();
    for (i, e) in p.entities.iter().enumerate() {
        w.put_facts(&e.id.0, i as u64, &facts(i)).unwrap();
    }
    w.finish(&p, sat_stll::file::keep_entities(&p)).unwrap();
    let again = t.elapsed();
    let t = Instant::now();
    let mut n = 0;
    for e in p.entities.iter() {
        n += usize::from(g.facts(&e.id.0).is_some());
    }
    println!(
        "{mods} mods × {files} files: first save {:.2} s, {:.1} MiB; open {:.0} ms; unchanged save {:.2} s; read all facts {:.2} s ({n})",
        first.as_secs_f64(),
        size as f64 / 1048576.0,
        open.as_secs_f64() * 1000.0,
        again.as_secs_f64(),
        t.elapsed().as_secs_f64()
    );
}
