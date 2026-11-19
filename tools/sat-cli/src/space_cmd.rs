//! `sat-cli space`: what the build's files take on disk and what could be won back, measured. @sat:cli.space
//!
//! Nothing on disk changes.
//! Owns: the measurement for the storage layer (owner, 2026-11-19: free the 2 TB without losing anything) —
//! identical copies across mods (same path, same size, same bytes: what hard links would win back, by class and by
//! how warm the copies are), DDS textures by format and size (uncompressed ones are the reserve), and the mods that
//! distributors name (SPID, BOS, KID, SkyPatcher: an outfit someone hands out without the player is not cold).
//! Not here: changing anything on disk (a later plan → check → backup → commit), the hashes themselves
//! (`@sat:scan.content-hash`).
//!
//! Every mod counts, enabled or not: what is off exists (PHILOSOPHY §4 «Существует ≠ активен»).

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::PathBuf;

use rayon::prelude::*;
use rustc_hash::FxHashMap;
use sat_mo2::Instance;
use sat_mo2::commit::Layout;

use crate::{flag_value, open_instance};

/// One loose file the measurement keeps.
struct File {
    m: u32,
    rel: Box<str>,
    size: u64,
}

/// One mod: its name, whether it is on, its separator.
struct Mod {
    name: String,
    on: bool,
    sep: String,
}

/// The class of a file by its extension (what it costs and how it compresses).
fn class(rel: &str) -> &'static str {
    let ext = rel.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "dds" | "png" | "tga" => "textures",
        "nif" | "tri" | "btr" | "bto" => "meshes",
        "hkx" => "animations",
        "wav" | "xwm" | "fuz" | "lip" | "mp3" | "ogg" => "sound",
        "bsa" | "ba2" => "archives",
        "pex" | "psc" => "scripts",
        "esp" | "esm" | "esl" => "plugins",
        "osd" | "osp" => "bodyslide",
        "7z" | "zip" | "rar" => "packed",
        _ => "other",
    }
}

/// Files a hard link may share: assets nothing writes in place (plugins, configs and texts are left alone).
fn linkable(rel: &str) -> bool {
    !matches!(class(rel), "plugins" | "other" | "packed")
}

// @sat:cli.space.run
/// `sat-cli space <instance> [--identical FILE] [--min-size SIZE] [--dds FILE] [--distributors FILE]`
pub fn space(args: &[String]) -> Result<(), String> {
    let inst = open_instance(args)?;
    let (mut stll, project) = sat_home::project::require(&Layout::of(&inst).project_file)?;
    let min = flag_value(args, "--min-size").map_or(Ok(64 * 1024), crate::parse_size)?;

    // mods in list order with their separator (a mod belongs to the separator above it)
    let mut mods: Vec<Mod> = Vec::new();
    let mut by_name: FxHashMap<String, u32> = FxHashMap::default();
    let mut sep = String::new();
    for e in &project.canon.entries {
        if e.is_mod() {
            by_name.insert(e.name.to_lowercase(), mods.len() as u32);
            mods.push(Mod {
                name: e.name.clone(),
                on: e.enabled,
                sep: sep.clone(),
            });
        } else {
            sep = e.name.trim_end_matches("_separator").to_string();
        }
    }
    // the files: one pass over the stored facts
    let ids: Vec<(String, String)> = project
        .entities
        .iter()
        .map(|e| (e.id.0.clone(), e.name.clone()))
        .collect();
    let mut files: Vec<File> = Vec::new();
    let mut plugin_owners: FxHashMap<String, Vec<u32>> = FxHashMap::default();
    let (mut all_files, mut all_bytes) = (0u64, 0u64);
    for (id, name) in &ids {
        let Some(&m) = by_name.get(&name.to_lowercase()) else {
            continue;
        };
        let Some((_, f)) = stll.facts(id) else { continue };
        for p in &f.plugins {
            plugin_owners
                .entry(p.file_name.to_lowercase())
                .or_default()
                .push(m);
        }
        for d in &f.dirs {
            for x in &d.files {
                all_files += 1;
                all_bytes += x.size;
                let rel = if d.rel.is_empty() {
                    x.name.clone()
                } else {
                    format!("{}/{}", d.rel, x.name)
                };
                let low = rel.to_ascii_lowercase();
                let keep = x.size >= min || low.ends_with(".dds") || distributor(&low);
                if keep {
                    files.push(File {
                        m,
                        rel: rel.into_boxed_str(),
                        size: x.size,
                    });
                }
            }
        }
    }
    drop(stll);
    eprintln!(
        "facts: {} mods, {all_files} loose files, {:.1} GB; kept {} for the measurement",
        mods.len(),
        all_bytes as f64 / GB,
        files.len()
    );
    let path_of = |f: &File| -> PathBuf { inst.mods_dir.join(&mods[f.m as usize].name).join(&*f.rel) };

    if let Some(out) = flag_value(args, "--identical") {
        identical(&files, &mods, &path_of, min, &inst, out)?;
    }
    if let Some(out) = flag_value(args, "--dds") {
        dds(&files, &mods, &path_of, out)?;
    }
    if let Some(out) = flag_value(args, "--distributors") {
        distributors(&files, &mods, &path_of, &plugin_owners, out)?;
    }
    Ok(())
}

const GB: f64 = 1024.0 * 1024.0 * 1024.0;

/// A distributor's config: SPID, BOS, KID, SkyPatcher.
fn distributor(low: &str) -> bool {
    low.ends_with("_distr.ini")
        || low.ends_with("_swap.ini")
        || low.ends_with("_kid.ini")
        || (low.contains("skypatcher/") && low.ends_with(".ini"))
}

// @sat:cli.space.identical
/// Copies with the same path and size in several mods, hashed (the profile's content-hash cache is shared with
/// `analyze`): what linking each set of equal copies to one file would win back.
fn identical(
    files: &[File],
    mods: &[Mod],
    path_of: &(dyn Fn(&File) -> PathBuf + Sync),
    min: u64,
    inst: &Instance,
    out: &str,
) -> Result<(), String> {
    use std::hash::BuildHasher;
    // the same path and size; a rare clash of the 64-bit key only merges two sets that the content hash splits again
    let key = |f: &File| {
        (
            rustc_hash::FxBuildHasher.hash_one(f.rel.to_ascii_lowercase()),
            f.size,
        )
    };
    let mut count: FxHashMap<(u64, u64), u32> = FxHashMap::default();
    for f in files.iter().filter(|f| f.size >= min) {
        *count.entry(key(f)).or_default() += 1;
    }
    let mut groups: FxHashMap<(u64, u64), Vec<usize>> = FxHashMap::default();
    for (i, f) in files.iter().enumerate() {
        if f.size >= min && count.get(&key(f)).is_some_and(|&c| c > 1) {
            groups.entry(key(f)).or_default().push(i);
        }
    }
    drop(count);
    let members: Vec<usize> = groups.values().flatten().copied().collect();
    let read_bytes: u64 = members.iter().map(|&i| files[i].size).sum();
    eprintln!(
        "identical: {} sets of the same path and size, {} copies, {:.1} GB to hash (cached ones are not read)",
        groups.len(),
        members.len(),
        read_bytes as f64 / GB
    );
    let cache_file = inst.profile_dir().join("satellite").join("content-hashes.txt");
    let mut cache = sat_scan::content::HashCache::load(&cache_file);
    let t = std::time::Instant::now();
    let mut hash: FxHashMap<usize, Option<u64>> = FxHashMap::default();
    // in chunks: the cache is saved as it goes, an interrupted run keeps what it read
    for chunk in members.chunks(20_000) {
        let paths: Vec<PathBuf> = chunk.iter().map(|&i| path_of(&files[i])).collect();
        let (h, read) = cache.hash_all(&paths);
        for (&i, h) in chunk.iter().zip(h) {
            hash.insert(i, h);
        }
        if read > 0 {
            cache
                .save(&cache_file)
                .map_err(|e| format!("{}: {e}", cache_file.display()))?;
        }
        eprintln!(
            "  hashed {}/{} ({read} read) in {:.0} s",
            hash.len(),
            members.len(),
            t.elapsed().as_secs_f64()
        );
    }
    // per set: copies with equal bytes; the win is every copy but one
    #[derive(Default)]
    struct Sum {
        sets: u64,
        copies: u64,
        win: u64,
    }
    let mut by_class: BTreeMap<(&str, &str), Sum> = BTreeMap::default();
    let mut by_sep: BTreeMap<String, u64> = BTreeMap::default();
    let mut w = std::io::BufWriter::new(std::fs::File::create(out).map_err(|e| format!("{out}: {e}"))?);
    let _ = writeln!(w, "size\tcopies\twin\tclass\twarmth\tlinkable\tpath\tmods");
    let (mut total, mut total_link, mut unread) = (0u64, 0u64, 0u64);
    for idx in groups.values() {
        let mut by_hash: FxHashMap<u64, Vec<usize>> = FxHashMap::default();
        for &i in idx {
            match hash.get(&i).copied().flatten() {
                Some(h) => by_hash.entry(h).or_default().push(i),
                None => unread += 1,
            }
        }
        for same in by_hash.values().filter(|v| v.len() > 1) {
            let f = &files[same[0]];
            let win = f.size * (same.len() as u64 - 1);
            let on = same.iter().filter(|&&i| mods[files[i].m as usize].on).count();
            // how warm: every copy in enabled mods, some, none
            let warmth = if on == same.len() {
                "all-on"
            } else if on > 0 {
                "mixed"
            } else {
                "all-off"
            };
            let link = linkable(&f.rel);
            let c = class(&f.rel);
            let s = by_class.entry((c, warmth)).or_default();
            s.sets += 1;
            s.copies += same.len() as u64;
            s.win += win;
            total += win;
            if link {
                total_link += win;
                *by_sep.entry(mods[f.m as usize].sep.clone()).or_default() += win;
            }
            let names: Vec<String> = same
                .iter()
                .map(|&i| {
                    let m = &mods[files[i].m as usize];
                    format!("{}{}", if m.on { "+" } else { "-" }, m.name)
                })
                .collect();
            let _ = writeln!(
                w,
                "{}\t{}\t{win}\t{c}\t{warmth}\t{link}\t{}\t{}",
                f.size,
                same.len(),
                f.rel,
                names.join(" | ")
            );
        }
    }
    w.flush().map_err(|e| e.to_string())?;
    println!(
        "identical copies: {:.1} GB could be won back, {:.1} GB of it in files a link may share (unreadable copies: {unread})",
        total as f64 / GB,
        total_link as f64 / GB
    );
    for ((c, warmth), s) in &by_class {
        println!(
            "  {c:10} {warmth:7} sets {:>8}  copies {:>8}  win {:>7.1} GB",
            s.sets,
            s.copies,
            s.win as f64 / GB
        );
    }
    let mut seps: Vec<(&String, &u64)> = by_sep.iter().collect();
    seps.sort_by_key(|s| std::cmp::Reverse(*s.1));
    for (s, b) in seps.iter().take(15) {
        println!(
            "  {:>7.1} GB  {}",
            **b as f64 / GB,
            if s.is_empty() { "(no separator)" } else { s }
        );
    }
    eprintln!("identical → {out}");
    Ok(())
}

// @sat:cli.space.dds
/// Every loose DDS by format and size, per separator: the headers are read, nothing else.
fn dds(
    files: &[File],
    mods: &[Mod],
    path_of: &(dyn Fn(&File) -> PathBuf + Sync),
    out: &str,
) -> Result<(), String> {
    let all: Vec<usize> = (0..files.len())
        .filter(|&i| files[i].rel.to_ascii_lowercase().ends_with(".dds"))
        .collect();
    let t = std::time::Instant::now();
    let info: Vec<(usize, Option<sat_formats::DdsInfo>)> = all
        .par_iter()
        .map(|&i| {
            let mut b = vec![0u8; sat_formats::dds::DDS_HEADER_PROBE];
            let got = std::fs::File::open(path_of(&files[i])).and_then(|mut f| {
                let n = f.read(&mut b)?;
                Ok(n)
            });
            (i, got.ok().and_then(|n| sat_formats::DdsInfo::parse(&b[..n])))
        })
        .collect();
    eprintln!(
        "dds: {} headers in {:.0} s",
        info.len(),
        t.elapsed().as_secs_f64()
    );
    // (separator, on, format, size bucket) → files, bytes
    let mut agg: BTreeMap<(String, bool, String, &str), (u64, u64)> = BTreeMap::default();
    let mut by_fmt: BTreeMap<String, (u64, u64)> = BTreeMap::default();
    for (i, d) in &info {
        let f = &files[*i];
        let m = &mods[f.m as usize];
        let (fmt, dim) = match d {
            Some(d) => (d.format.clone(), d.width.max(d.height)),
            None => ("unreadable".to_string(), 0),
        };
        let bucket = match dim {
            0 => "?",
            1..=512 => "≤512",
            513..=1024 => "1K",
            1025..=2048 => "2K",
            2049..=4096 => "4K",
            _ => "8K+",
        };
        let a = agg.entry((m.sep.clone(), m.on, fmt.clone(), bucket)).or_default();
        a.0 += 1;
        a.1 += f.size;
        let b = by_fmt.entry(format!("{fmt} {bucket}")).or_default();
        b.0 += 1;
        b.1 += f.size;
    }
    let mut w = std::io::BufWriter::new(std::fs::File::create(out).map_err(|e| format!("{out}: {e}"))?);
    let _ = writeln!(w, "separator\tenabled\tformat\tsize\tfiles\tbytes");
    for ((s, on, fmt, b), (n, bytes)) in &agg {
        let _ = writeln!(w, "{s}\t{on}\t{fmt}\t{b}\t{n}\t{bytes}");
    }
    w.flush().map_err(|e| e.to_string())?;
    let mut rows: Vec<(&String, &(u64, u64))> = by_fmt.iter().collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.1.1));
    println!("dds by format and size:");
    for (k, (n, b)) in rows.iter().take(25) {
        println!("  {:>7.1} GB {n:>9} files  {k}", *b as f64 / GB);
    }
    eprintln!("dds → {out}");
    Ok(())
}

// @sat:cli.space.distributors
/// The plugins that distributor configs name, and the mods that carry them: content handed out without the player.
fn distributors(
    files: &[File],
    mods: &[Mod],
    path_of: &(dyn Fn(&File) -> PathBuf + Sync),
    plugin_owners: &FxHashMap<String, Vec<u32>>,
    out: &str,
) -> Result<(), String> {
    let mut w = std::io::BufWriter::new(std::fs::File::create(out).map_err(|e| format!("{out}: {e}"))?);
    let _ = writeln!(w, "config_mod\tconfig_on\tconfig\tplugin\tplugin_mods");
    let (mut configs, mut refs) = (0u32, 0u32);
    for f in files.iter().filter(|f| distributor(&f.rel.to_ascii_lowercase())) {
        let Ok(bytes) = std::fs::read(path_of(f)) else {
            continue;
        };
        configs += 1;
        let text = String::from_utf8_lossy(&bytes);
        let mut named: Vec<String> = Vec::new();
        for line in text.lines() {
            let line = line.split(';').next().unwrap_or("");
            // a plugin name: the text before `.esp` / `.esm` / `.esl` back to a separator
            let low = line.to_ascii_lowercase();
            for ext in [".esp", ".esm", ".esl"] {
                let mut from = 0;
                while let Some(p) = low[from..].find(ext) {
                    let end = from + p + ext.len();
                    let start = line[..from + p]
                        .rfind(['~', '|', ',', '=', '"', '\t'])
                        .map_or(0, |s| s + 1);
                    let name = line[start..end].trim().to_string();
                    if !name.is_empty() && !named.iter().any(|n| n.eq_ignore_ascii_case(&name)) {
                        named.push(name);
                    }
                    from = end;
                }
            }
        }
        let m = &mods[f.m as usize];
        for n in named {
            refs += 1;
            let owners: Vec<String> = plugin_owners
                .get(&n.to_lowercase())
                .map(|v| {
                    v.iter()
                        .map(|&i| {
                            let o = &mods[i as usize];
                            format!("{}{}", if o.on { "+" } else { "-" }, o.name)
                        })
                        .collect()
                })
                .unwrap_or_default();
            let _ = writeln!(w, "{}\t{}\t{}\t{n}\t{}", m.name, m.on, f.rel, owners.join(" | "));
        }
    }
    w.flush().map_err(|e| e.to_string())?;
    println!("distributors: {configs} configs name {refs} plugins");
    eprintln!("distributors → {out}");
    Ok(())
}
