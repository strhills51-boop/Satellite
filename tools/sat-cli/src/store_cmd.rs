//! `sat-cli store`: the storage layer from the command line — the plan, applying it, undoing a mod. @sat:cli.store
//!
//! Owns: building the plan from the stored facts and the disk (identical copies through the shared content-hash
//! cache, which copy is already one file with which, what is compressed), writing it, applying it — links first
//! (byte for byte, each new link locked), then compression in parallel batches — with the journal; undoing a mod,
//! freeing a mod for editing by hand, keeping the locks of shared files.
//! Not here: the rules (`@sat:storage`), the disk work (`@sat:mo2.storage`), finding the files (`@sat:cli.space.load`).
//!
//! ADR 0022. `--scope disabled` (the default) never touches a file of an enabled mod: copies of disabled mods become
//! links to an enabled keeper, the rest of the disabled mods is compressed. `--scope all` also links and compresses
//! the files the game reads; it refuses while MO2 is open.

use std::hash::BuildHasher;
use std::io::Write;
use std::path::PathBuf;

use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};
use sat_core::storage::{self, Codec, Copy, Policy, Scope, Tier};
use sat_mo2::storage as disk;

use crate::space_cmd::{File, GB, load};
use crate::{flag_value, flag_values, has, open_instance};

/// One step of the plan.
enum Op {
    /// Replace file `copy` by a link to file `keeper`.
    Link {
        keeper: usize,
        copy: usize,
    },
    Compress {
        file: usize,
        codec: Codec,
        tier: Tier,
    },
}

// @sat:cli.store.run
/// `sat-cli store <instance> [--scope disabled|all] [--tiers cold,warm,hot] [--no-links] [--no-compress]
/// [--plan FILE] [--apply [--limit-gb N] [--jobs N] [--no-lock]] [--undo MOD]... [--free MOD]... [--lock | --unlock]`
pub fn store(args: &[String]) -> Result<(), String> {
    let inst = open_instance(args)?;
    let policy = Policy::default();
    let journal = inst.profile_dir().join("satellite").join("storage-journal.tsv");

    let undo_mods = flag_values(args, "--undo");
    for m in &undo_mods {
        undo(&inst.mods_dir, &journal, m)?;
    }
    let free_mods = flag_values(args, "--free");
    for m in &free_mods {
        free(&inst.mods_dir, &journal, m)?;
    }
    if has(args, "--lock") || has(args, "--unlock") {
        locks(&inst.mods_dir, &journal, has(args, "--lock"));
    }
    let upkeep =
        !undo_mods.is_empty() || !free_mods.is_empty() || has(args, "--lock") || has(args, "--unlock");
    if upkeep && !has(args, "--plan") && !has(args, "--apply") {
        return Ok(());
    }

    let scope = match flag_value(args, "--scope") {
        Some("all") => Scope::All,
        Some("disabled") | None => Scope::Disabled,
        Some(s) => return Err(format!("--scope {s}: disabled | all")),
    };
    let tiers: Vec<Tier> = match flag_value(args, "--tiers") {
        Some(list) => list
            .split(',')
            .map(|t| match t.trim() {
                "hot" => Ok(Tier::Hot),
                "warm" => Ok(Tier::Warm),
                "cold" => Ok(Tier::Cold),
                o => Err(format!("--tiers {o}: hot | warm | cold")),
            })
            .collect::<Result<_, _>>()?,
        None if scope == Scope::Disabled => vec![Tier::Cold],
        None => vec![Tier::Hot, Tier::Warm, Tier::Cold],
    };
    let (mods, files, _) = load(&inst, |low, size| {
        size >= policy.compress_min && storage::class(low).compressible()
    })?;
    let touch: Vec<bool> = mods.iter().map(|m| !policy.untouched(&m.sep)).collect();
    let tier_of: Vec<Tier> = mods.iter().map(|m| storage::mod_tier(m.on, &m.tags).0).collect();
    let path_of = |f: &File| -> PathBuf { inst.mods_dir.join(&mods[f.m as usize].name).join(&*f.rel) };
    let rel_of = |f: &File| format!("{}/{}", mods[f.m as usize].name, f.rel);
    // a file of an enabled mod is the game's: the first pass leaves it as it is
    let may = |f: &File| touch[f.m as usize] && (scope == Scope::All || !mods[f.m as usize].on);

    let mut ops: Vec<Op> = Vec::new();
    // files whose bytes are (or become) another file's: compressed through their keeper only
    let mut covered: FxHashSet<usize> = FxHashSet::default();
    let mut set_tier: FxHashMap<usize, Tier> = FxHashMap::default();

    // 1. links: copies with the same path, size and bytes
    if !has(args, "--no-links") {
        let key = |f: &File| {
            (
                rustc_hash::FxBuildHasher.hash_one(f.rel.to_ascii_lowercase()),
                f.size,
            )
        };
        // a path a generator writes may be written through MO2 into whichever mod has it: never a link
        let generated: FxHashSet<String> = files
            .iter()
            .filter(|f| !touch[f.m as usize])
            .map(|f| f.rel.to_ascii_lowercase())
            .collect();
        let mut groups: FxHashMap<(u64, u64), Vec<usize>> = FxHashMap::default();
        for (i, f) in files.iter().enumerate() {
            if f.size >= policy.link_min
                && storage::class(&f.rel).linkable()
                && touch[f.m as usize]
                && !generated.contains(&f.rel.to_ascii_lowercase())
            {
                groups.entry(key(f)).or_default().push(i);
            }
        }
        groups.retain(|_, v| v.len() > 1);
        let members: Vec<usize> = groups.values().flatten().copied().collect();
        eprintln!(
            "links: {} sets of the same path and size, {} copies",
            groups.len(),
            members.len()
        );
        let cache_file = inst.profile_dir().join("satellite").join("content-hashes.txt");
        let mut cache = sat_scan::content::HashCache::load(&cache_file);
        let mut hash: FxHashMap<usize, Option<u64>> = FxHashMap::default();
        for chunk in members.chunks(20_000) {
            let paths: Vec<PathBuf> = chunk.iter().map(|&i| path_of(&files[i])).collect();
            let (h, read) = cache.hash_all(&paths);
            hash.extend(chunk.iter().copied().zip(h));
            if read > 0 {
                cache
                    .save(&cache_file)
                    .map_err(|e| format!("{}: {e}", cache_file.display()))?;
            }
        }
        // which file each copy is on disk (already linked copies are one file)
        let ids: FxHashMap<usize, u128> = members
            .par_iter()
            .map(|&i| (i, disk::file_id(&path_of(&files[i]))))
            .collect();
        for idx in groups.values() {
            let mut by_hash: FxHashMap<u64, Vec<usize>> = FxHashMap::default();
            for &i in idx {
                if let Some(Some(h)) = hash.get(&i) {
                    by_hash.entry(*h).or_default().push(i);
                }
            }
            for same in by_hash.values().filter(|v| v.len() > 1) {
                let copies: Vec<Copy> = same
                    .iter()
                    .map(|&i| {
                        let m = files[i].m as usize;
                        Copy {
                            enabled: mods[m].on,
                            tier: tier_of[m],
                            file: ids.get(&i).copied().unwrap_or(0),
                        }
                    })
                    .collect();
                // the keeper's bytes go on being read by every copy already linked to it
                let keeper_at = copies.iter().position(|c| c.enabled).unwrap_or(0);
                let kid = copies[keeper_at].file;
                for (j, c) in copies.iter().enumerate() {
                    if j != keeper_at && kid != 0 && c.file == kid {
                        covered.insert(same[j]);
                    }
                }
                let Some(plan) = storage::link_set(&copies, scope) else {
                    continue;
                };
                let keeper = same[plan.keeper];
                set_tier.insert(keeper, plan.tier);
                for r in plan.replace {
                    covered.insert(same[r]);
                    ops.push(Op::Link {
                        keeper,
                        copy: same[r],
                    });
                }
            }
        }
    }

    // 2. compression by temperature
    if !has(args, "--no-compress") {
        let todo: Vec<(usize, Tier)> = files
            .iter()
            .enumerate()
            .filter(|(i, f)| may(f) && !covered.contains(i))
            .map(|(i, f)| (i, set_tier.get(&i).copied().unwrap_or(tier_of[f.m as usize])))
            .filter(|(_, t)| tiers.contains(t))
            .collect();
        // files compression left as they were last time (it did not shrink them): not tried again at the same size
        let kept: FxHashMap<String, u64> = {
            let mut last: FxHashMap<String, disk::JournalLine> = FxHashMap::default();
            for l in disk::journal_read(&journal) {
                last.insert(l.path.to_lowercase(), l);
            }
            last.into_iter()
                .filter(|(_, l)| l.op == "kept")
                .map(|(p, l)| (p, l.size))
                .collect()
        };
        eprintln!(
            "compression: checking {} files on disk ({} left as they were before)",
            todo.len(),
            kept.len()
        );
        let found: Vec<(usize, Tier, Option<Codec>)> = todo
            .par_iter()
            .map(|&(i, t)| {
                let f = &files[i];
                if kept.get(&rel_of(f).to_lowercase()) == Some(&f.size) {
                    return (i, t, None);
                }
                // the WOF filter hides the reparse point: a compressed file is one smaller on disk than its size
                let done = disk::compressed_at(&path_of(f), f.size);
                (
                    i,
                    t,
                    storage::needs(&policy, storage::class(&f.rel), f.size, t, done),
                )
            })
            .collect();
        for (i, tier, c) in found {
            if let Some(codec) = c {
                ops.push(Op::Compress { file: i, codec, tier });
            }
        }
    }

    // 3. what the plan wins
    let (mut links, mut link_bytes) = (0u64, 0u64);
    let mut comp: std::collections::BTreeMap<(String, String), (u64, u64, f64)> = Default::default();
    for op in &ops {
        match op {
            Op::Link { copy, .. } => {
                links += 1;
                link_bytes += files[*copy].size;
            }
            Op::Compress { file, codec, tier } => {
                let f = &files[*file];
                let c = storage::class(&f.rel);
                let e = comp
                    .entry((format!("{tier:?}"), format!("{c:?} {}", codec.compact_name())))
                    .or_default();
                e.0 += 1;
                e.1 += f.size;
                e.2 += f.size as f64 * (1.0 - c.kept(*codec));
            }
        }
    }
    println!(
        "plan ({}): links {links} copies, {:.1} GB back",
        if scope == Scope::Disabled {
            "disabled mods only"
        } else {
            "all mods"
        },
        link_bytes as f64 / GB
    );
    let mut est = 0.0;
    for ((tier, what), (n, bytes, win)) in &comp {
        println!(
            "  compress {tier:5} {what:24} {n:>9} files {:>8.1} GB → about {:>7.1} GB back",
            *bytes as f64 / GB,
            win / GB
        );
        est += win;
    }
    println!(
        "  in all about {:.1} GB back (compression is an estimate from the sample)",
        (link_bytes as f64 + est) / GB
    );
    if let Some(out) = flag_value(args, "--plan") {
        let mut w = std::io::BufWriter::new(std::fs::File::create(out).map_err(|e| format!("{out}: {e}"))?);
        let _ = writeln!(w, "op\tcodec\ttier\tpath\tsize\tkeeper");
        for op in &ops {
            let _ = match op {
                Op::Link { keeper, copy } => writeln!(
                    w,
                    "link\t\t\t{}\t{}\t{}",
                    rel_of(&files[*copy]),
                    files[*copy].size,
                    rel_of(&files[*keeper])
                ),
                Op::Compress { file, codec, tier } => writeln!(
                    w,
                    "compress\t{}\t{tier:?}\t{}\t{}\t",
                    codec.compact_name(),
                    rel_of(&files[*file]),
                    files[*file].size
                ),
            };
        }
        w.flush().map_err(|e| e.to_string())?;
        eprintln!("plan → {out}");
    }

    // 4. apply
    if !has(args, "--apply") {
        return Ok(());
    }
    if scope == Scope::All && sat_mo2::processors::mo2_running() {
        return Err("MO2 is open: close it before touching the files of enabled mods".into());
    }
    let limit = flag_value(args, "--limit-gb")
        .map(|v| v.parse::<f64>().map_err(|_| format!("--limit-gb {v}")))
        .transpose()?
        .map(|g| (g * GB) as u64);
    let jobs: usize = flag_value(args, "--jobs")
        .and_then(|v| v.parse().ok())
        .unwrap_or(2)
        .max(1);
    let t = std::time::Instant::now();
    let mut done_bytes = 0u64;
    let mut failed: Vec<String> = Vec::new();

    let lock = !has(args, "--no-lock");
    let mut j = disk::Journal::open(&journal).map_err(|e| format!("{}: {e}", journal.display()))?;
    // links: one by one, each checked byte for byte, locked and journaled as soon as it is made
    let mut linked = 0u64;
    for op in &ops {
        let Op::Link { keeper, copy } = op else {
            continue;
        };
        if limit.is_some_and(|l| done_bytes >= l) {
            break;
        }
        let (k, c) = (&files[*keeper], &files[*copy]);
        match disk::link_copy(&path_of(k), &path_of(c)) {
            Ok(()) => {
                linked += 1;
                done_bytes += c.size;
                if lock {
                    let _ = disk::set_locked(&path_of(c), true);
                }
                j.write(&disk::JournalLine {
                    time: disk::now(),
                    op: "link".into(),
                    codec: String::new(),
                    path: rel_of(c),
                    keeper: rel_of(k),
                    size: c.size,
                })
                .map_err(|e| e.to_string())?;
            }
            Err(e) => failed.push(e),
        }
        if linked > 0 && linked.is_multiple_of(1000) {
            eprintln!(
                "  linked {linked} ({:.1} GB) in {:.0} s",
                done_bytes as f64 / GB,
                t.elapsed().as_secs_f64()
            );
        }
    }
    eprintln!("links: {linked} made, {} failed", failed.len());

    // compression: batches per codec, `jobs` at a time; the journal after every round
    let mut by_codec: FxHashMap<&'static str, Vec<usize>> = FxHashMap::default();
    for op in &ops {
        if let Op::Compress { file, codec, .. } = op {
            by_codec.entry(codec.compact_name()).or_default().push(*file);
        }
    }
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(jobs)
        .build()
        .map_err(|e| e.to_string())?;
    let (mut compressed, mut left) = (0u64, 0u64);
    'codecs: for (name, list) in by_codec {
        let codec = Codec::from_name(name).expect("a known codec");
        for round in list.chunks(200 * jobs) {
            if limit.is_some_and(|l| done_bytes >= l) {
                break 'codecs;
            }
            let batches: Vec<Vec<usize>> = round.chunks(200).map(<[usize]>::to_vec).collect();
            let results: Vec<Result<Vec<(PathBuf, bool)>, String>> = pool.install(|| {
                batches
                    .par_iter()
                    .map(|b| {
                        let paths: Vec<PathBuf> = b.iter().map(|&i| path_of(&files[i])).collect();
                        disk::compact(&paths, Some(codec))
                    })
                    .collect()
            });
            for (b, r) in batches.iter().zip(results) {
                let r = r?;
                for (&i, (_, ok)) in b.iter().zip(r) {
                    let f = &files[i];
                    // every file tried counts toward the limit, shrunk or not
                    done_bytes += f.size;
                    if ok {
                        compressed += 1;
                    } else {
                        left += 1;
                    }
                    // `kept`: compression did not shrink it (or could not touch it) — the plan skips it next time
                    j.write(&disk::JournalLine {
                        time: disk::now(),
                        op: if ok { "compress" } else { "kept" }.into(),
                        codec: name.to_string(),
                        path: rel_of(f),
                        keeper: String::new(),
                        size: f.size,
                    })
                    .map_err(|e| e.to_string())?;
                }
            }
            eprintln!(
                "  compressed {compressed} files, {left} left as they were ({:.1} GB processed) in {:.0} s",
                done_bytes as f64 / GB,
                t.elapsed().as_secs_f64()
            );
        }
    }
    println!(
        "applied: {linked} links, {compressed} files compressed, {left} left as they were, {:.1} GB processed in \
         {:.0} min; {} failed",
        done_bytes as f64 / GB,
        t.elapsed().as_secs_f64() / 60.0,
        failed.len()
    );
    for f in failed.iter().take(20) {
        eprintln!("  {f}");
    }
    eprintln!("journal → {}", journal.display());
    Ok(())
}

// @sat:cli.store.undo
/// Undo what Satellite did to the files of mod `name`: the links the journal names become files of their own again
/// (their newest line is a link), and every WOF-compressed file of the mod is decompressed — the disk says which,
/// so a run cut short before its journal line is undone too.
fn undo(mods_dir: &std::path::Path, journal: &std::path::Path, name: &str) -> Result<(), String> {
    let dir = mod_dir(mods_dir, name, "--undo")?;
    let prefix = format!("{}/", name.to_lowercase());
    let mut last: FxHashMap<String, disk::JournalLine> = FxHashMap::default();
    for l in disk::journal_read(journal) {
        if l.path.to_lowercase().starts_with(&prefix) {
            last.insert(l.path.to_lowercase(), l);
        }
    }
    let mut j = disk::Journal::open(journal).map_err(|e| format!("{}: {e}", journal.display()))?;
    let mut unlinked = 0u32;
    for l in last.into_values().filter(|l| l.op == "link") {
        match disk::unlink_copy(&mods_dir.join(&l.path), Some(&mods_dir.join(&l.keeper))) {
            Ok(()) => {
                unlinked += 1;
                j.write(&disk::JournalLine {
                    time: disk::now(),
                    op: "unlink".into(),
                    ..l
                })
                .map_err(|e| e.to_string())?;
            }
            Err(e) => eprintln!("  {e}"),
        }
    }
    let packed = disk::compressed_under(&dir);
    let mut back = 0u32;
    if !packed.is_empty() {
        for (p, ok) in disk::compact(&packed, None)? {
            if !ok {
                eprintln!("  {}: still compressed", p.display());
                continue;
            }
            back += 1;
            let rel = p
                .strip_prefix(mods_dir)
                .unwrap_or(&p)
                .to_string_lossy()
                .replace('\\', "/");
            j.write(&disk::JournalLine {
                time: disk::now(),
                op: "decompress".into(),
                codec: String::new(),
                path: rel,
                keeper: String::new(),
                size: std::fs::metadata(&p).map_or(0, |m| m.len()),
            })
            .map_err(|e| e.to_string())?;
        }
    }
    println!("{name}: {unlinked} links made files again, {back} files decompressed");
    Ok(())
}

/// One mod folder by its name: never the mods folder itself or a path out of it.
fn mod_dir(mods_dir: &std::path::Path, name: &str, flag: &str) -> Result<PathBuf, String> {
    if name.trim().is_empty() || name.contains(['/', '\\']) || name == "." || name == ".." {
        return Err(format!("{flag} {name:?}: the name of one mod folder"));
    }
    let dir = mods_dir.join(name);
    if !dir.is_dir() {
        return Err(format!("{flag} {name}: no such mod folder"));
    }
    Ok(dir)
}

/// The links the journal says are standing: the newest line of each path is a link.
fn links_now(journal: &std::path::Path) -> Vec<disk::JournalLine> {
    let mut last: FxHashMap<String, disk::JournalLine> = FxHashMap::default();
    for l in disk::journal_read(journal) {
        last.insert(l.path.to_lowercase(), l);
    }
    last.into_values().filter(|l| l.op == "link").collect()
}

// @sat:cli.store.free
/// Free mod `name` for editing by hand: every file of it that shares its bytes with another mod becomes a file of
/// its own, without a lock — a copy here leaves its keeper; a keeper here leaves its copies (they stay one file among
/// themselves, locked). Compression stays: writing to a WOF file decompresses it. The next pass decides anew.
fn free(mods_dir: &std::path::Path, journal: &std::path::Path, name: &str) -> Result<(), String> {
    mod_dir(mods_dir, name, "--free")?;
    let prefix = format!("{}/", name.to_lowercase());
    let mine = |p: &str| p.to_lowercase().starts_with(&prefix);
    let mut j = disk::Journal::open(journal).map_err(|e| format!("{}: {e}", journal.display()))?;
    let (mut copies, mut keepers) = (0u32, 0u32);
    let mut kept_by: FxHashMap<String, (String, Vec<String>)> = FxHashMap::default();
    for l in links_now(journal) {
        if mine(&l.path) {
            match disk::unlink_copy(&mods_dir.join(&l.path), Some(&mods_dir.join(&l.keeper))) {
                Ok(()) => {
                    copies += 1;
                    j.write(&disk::JournalLine {
                        time: disk::now(),
                        op: "unlink".into(),
                        ..l
                    })
                    .map_err(|e| e.to_string())?;
                }
                Err(e) => eprintln!("  {e}"),
            }
        } else if mine(&l.keeper) {
            let e = kept_by
                .entry(l.keeper.to_lowercase())
                .or_insert_with(|| (l.keeper.clone(), Vec::new()));
            e.1.push(l.path);
        }
    }
    for (keeper, others) in kept_by.into_values() {
        let path = mods_dir.join(&keeper);
        // only while the keeper is still one file with its copies
        let id = disk::file_id(&path);
        let Some(partner) = others
            .iter()
            .map(|o| mods_dir.join(o))
            .find(|o| id != 0 && disk::file_id(o) == id)
        else {
            continue;
        };
        match disk::unlink_copy(&path, Some(&partner)) {
            Ok(()) => {
                keepers += 1;
                j.write(&disk::JournalLine {
                    time: disk::now(),
                    op: "free".into(),
                    codec: String::new(),
                    path: keeper,
                    keeper: String::new(),
                    size: std::fs::metadata(&path).map_or(0, |m| m.len()),
                })
                .map_err(|e| e.to_string())?;
            }
            Err(e) => eprintln!("  {e}"),
        }
    }
    println!("{name}: free for editing — {copies} copies and {keepers} keepers are files of their own now");
    Ok(())
}

// @sat:cli.store.lock
/// Keep the locks true to the disk: every name of a file shared by two or more names the journal knows is locked
/// (`--lock`); a name no longer shared is unlocked. `--unlock` lifts every lock the journal's links could carry.
fn locks(mods_dir: &std::path::Path, journal: &std::path::Path, on: bool) {
    let mut names: FxHashSet<String> = FxHashSet::default();
    let mut paths: Vec<PathBuf> = Vec::new();
    for l in links_now(journal) {
        for p in [&l.path, &l.keeper] {
            if names.insert(p.to_lowercase()) {
                paths.push(mods_dir.join(p));
            }
        }
    }
    let ids: Vec<u128> = paths.par_iter().map(|p| disk::file_id(p)).collect();
    let mut count: FxHashMap<u128, u32> = FxHashMap::default();
    for &id in ids.iter().filter(|&&id| id != 0) {
        *count.entry(id).or_default() += 1;
    }
    let (locked, unlocked) = paths
        .par_iter()
        .zip(&ids)
        .filter(|(_, id)| **id != 0)
        .map(|(p, id)| {
            let want = on && count[id] > 1;
            match (disk::is_locked(p), want) {
                (false, true) => (u32::from(disk::set_locked(p, true).is_ok()), 0),
                (true, false) => (0, u32::from(disk::set_locked(p, false).is_ok())),
                _ => (0, 0),
            }
        })
        .reduce(|| (0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
    println!(
        "locks: {} names known, {locked} locked now, {unlocked} unlocked",
        paths.len()
    );
}
