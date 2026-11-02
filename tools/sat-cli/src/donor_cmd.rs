//! `sat-cli donor`: an external donor build against the project, and taking its mods into the plan. @sat:cli.donor
//!
//! Owns: the donor session on the command line (monolith `ExternalDonorSession`: fit of the donor's modlist to its
//! folder, candidates by kind against the project), binding the donor to this machine, and taking candidates — they
//! become planned mods of the project (their place in the order now, their folders at the next commit).
//! Not here: matching (`@sat:sync.donor`), the machine binding and the copy (`@sat:mo2.donors`), the commit
//! (`@sat:cli.commit`, `@sat:mo2.commit.donor`).
//!
//! The monolith never moved donor folders (its own map calls it a contract gap); here taking is an intent, so the
//! commit carries it out with its backup and rollback, and a planned mod's conflicts are seen before it arrives.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Serialize;

use sat_core::canon::{EntryKind, Modlist, SEPARATOR_SUFFIX};
use sat_core::intent::Source;
use sat_core::sync::donor::{self, MatchStatus};
use sat_core::sync::names::is_separator_folder;
use sat_mo2::commit::Layout;
use sat_mo2::profile;

use crate::{flag_value, flag_values, has, open_instance, project_io, subfolders};

#[derive(Serialize)]
struct DonorReport {
    target_mods: usize,
    donor: String,
    donor_mods_dir: String,
    donor_modlist: Option<String>,
    validation: Option<donor::Validation>,
    counts: BTreeMap<String, usize>,
    candidates: Vec<donor::Candidate>,
}

fn read_modlist(path: &std::path::Path) -> Result<Modlist, String> {
    let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(profile::parse_modlist(&data).0)
}

/// The mods of a canon with their separators (the donor's targets).
fn targets(list: &Modlist) -> donor::TargetIndex {
    let mut sep = String::new();
    let mut out = Vec::new();
    for e in &list.entries {
        match e.kind {
            EntryKind::Separator => sep = e.name.trim_end_matches(SEPARATOR_SUFFIX).to_string(),
            EntryKind::Mod => out.push(donor::TargetMod {
                name: e.name.clone(),
                separator: sep.clone(),
            }),
            EntryKind::Foreign => {}
        }
    }
    donor::TargetIndex::new(out)
}

/// The entry a mod goes after to land at the end of separator `sep` (its last entry, or the separator itself).
fn end_of_separator(list: &Modlist, sep: &str) -> Option<String> {
    let want = sep.trim_end_matches(SEPARATOR_SUFFIX).to_lowercase();
    let at = list.entries.iter().position(|e| {
        e.kind == EntryKind::Separator && e.name.trim_end_matches(SEPARATOR_SUFFIX).to_lowercase() == want
    })?;
    let end = list.entries[at + 1..]
        .iter()
        .position(|e| e.kind == EntryKind::Separator)
        .map_or(list.entries.len(), |i| at + 1 + i);
    Some(list.entries[end - 1].name.clone())
}

/// `sat-cli donor <instance> --donor-mods DIR [--donor-modlist FILE] [--as NAME] [--list] [--report FILE]
/// [--take NAME]... [--take-ready] [--separator SEP | --after MOD] [--move]`
pub fn donor(args: &[String]) -> Result<(), String> {
    let inst = open_instance(args)?;
    let layout = Layout::of(&inst);
    let mods_dir = PathBuf::from(flag_value(args, "--donor-mods").ok_or("missing --donor-mods DIR")?);
    if !mods_dir.is_dir() {
        return Err(format!("not a folder: {}", mods_dir.display()));
    }
    // the donor's name: given, or the build folder above its mods folder
    let name = flag_value(args, "--as").map(str::to_string).unwrap_or_else(|| {
        let base = mods_dir.parent().unwrap_or(&mods_dir);
        base.file_name()
            .map_or_else(|| "donor".to_string(), |n| n.to_string_lossy().into_owned())
    });
    let opened = project_io::open(&layout.project_file)?;
    let canon = match &opened {
        Some((_, p)) => p.canon.clone(),
        None => read_modlist(&inst.profile_dir().join("modlist.txt"))?,
    };
    let target = targets(&canon);
    let folders = subfolders(&mods_dir);
    let donor_list = match flag_value(args, "--donor-modlist") {
        Some(p) => read_modlist(std::path::Path::new(p))?,
        None => Modlist::default(),
    };
    let validation = flag_value(args, "--donor-modlist").map(|_| donor::validate(&donor_list, &folders));
    let candidates = donor::candidates(&donor_list, &folders, &target);
    let mut counts = BTreeMap::new();
    for c in &candidates {
        *counts.entry(format!("{:?}", c.kind).to_lowercase()).or_insert(0) += 1;
    }
    eprintln!(
        "donor {name:?} at {}: folders={} (separators {}); target: {} ({} mods)",
        mods_dir.display(),
        folders.len(),
        folders.iter().filter(|f| is_separator_folder(f)).count(),
        if opened.is_some() {
            "the project"
        } else {
            "modlist.txt"
        },
        target.targets.len()
    );
    if let Some(v) = &validation {
        eprintln!(
            "donor modlist fit: {}% ({}) matched={}/{} folders={} modlist_only={} folder_only={}",
            v.score,
            v.tone(),
            v.matched,
            v.modlist_total,
            v.folder_total,
            v.modlist_only.len(),
            v.folder_only.len()
        );
    }
    eprintln!("candidates: {counts:?}");
    // @sat:cli.donor.drained
    // draining: per donor separator, folders the project still lacks; none left — the separator can go
    if flag_value(args, "--donor-modlist").is_some() {
        let mut left: BTreeMap<&str, usize> = BTreeMap::new();
        for c in candidates.iter().filter(|c| c.has_modlist_entry) {
            *left.entry(c.source_separator.as_str()).or_default() +=
                usize::from(c.folder.is_some() && c.status != MatchStatus::Exact);
        }
        let drained: Vec<&str> = left.iter().filter(|x| *x.1 == 0).map(|x| *x.0).collect();
        let open: Vec<String> = left
            .iter()
            .filter(|x| *x.1 > 0)
            .map(|(s, n)| format!("{s} {n}"))
            .collect();
        eprintln!(
            "donor separators still to take from: {}",
            if open.is_empty() {
                "none".into()
            } else {
                open.join(", ")
            }
        );
        if !drained.is_empty() {
            eprintln!(
                "drained (everything is in the project or gone): {}",
                drained.join(", ")
            );
        }
    }
    if has(args, "--list") {
        for c in &candidates {
            println!(
                "{:<11} {}   [{}]{}",
                format!("{:?}", c.kind).to_lowercase(),
                c.name,
                c.source_separator,
                if c.enabled { "" } else { "  (off in the donor)" }
            );
        }
    }
    if let Some(out) = flag_value(args, "--report") {
        let report = DonorReport {
            target_mods: target.targets.len(),
            donor: name.clone(),
            donor_mods_dir: mods_dir.display().to_string(),
            donor_modlist: flag_value(args, "--donor-modlist").map(str::to_string),
            validation,
            counts,
            candidates: candidates.clone(),
        };
        std::fs::write(
            out,
            serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
    }

    // @sat:cli.donor.take
    let named = flag_values(args, "--take");
    if named.is_empty() && !has(args, "--take-ready") {
        if flag_value(args, "--as").is_some() {
            sat_mo2::donors::bind(&inst.profile_dir(), &name, &mods_dir)?;
            eprintln!("bound: donor {name:?} → {}", mods_dir.display());
        }
        return Ok(());
    }
    let (mut stll, mut project) = opened.ok_or("no project — run `sat-cli analyze` first")?;
    let mut take: Vec<&donor::Candidate> = Vec::new();
    for n in &named {
        let c = candidates
            .iter()
            .find(|c| {
                c.name.eq_ignore_ascii_case(n)
                    || c.folder.as_deref().is_some_and(|f| f.eq_ignore_ascii_case(n))
            })
            .ok_or_else(|| format!("--take {n}: not a mod of the donor"))?;
        match c.status {
            MatchStatus::FolderMissing => return Err(format!("{n}: the donor has no folder for it")),
            MatchStatus::Exact => return Err(format!("{n}: already in the project")),
            // a similar name is taken when asked by name: the user looked
            MatchStatus::Contains | MatchStatus::DonorOnly => take.push(c),
        }
    }
    if has(args, "--take-ready") {
        // every donor folder the project does not have, listed in the donor's modlist or not
        let named: Vec<String> = take.iter().map(|t| t.name.clone()).collect();
        take.extend(candidates.iter().filter(|c| {
            c.status == MatchStatus::DonorOnly && c.folder.is_some() && !named.contains(&c.name)
        }));
    }
    let mut after = match (flag_value(args, "--separator"), flag_value(args, "--after")) {
        (Some(s), _) => Some(
            end_of_separator(&project.canon, s)
                .ok_or_else(|| format!("--separator {s}: not in the project"))?,
        ),
        (None, a) => a.map(str::to_string),
    };
    let moved = has(args, "--move");
    sat_mo2::donors::bind(&inst.profile_dir(), &name, &mods_dir)?;
    // the folders are scanned now: the planned mods' conflicts show at the next analysis
    let jobs: Vec<sat_scan::ScanJob> = take
        .iter()
        .map(|c| sat_scan::ScanJob {
            name: c.name.clone(),
            path: mods_dir.join(c.folder.as_deref().unwrap_or(&c.name)),
        })
        .collect();
    let mut facts: Vec<Option<sat_core::facts::ModFacts>> = vec![None; jobs.len()];
    let opts = sat_scan::ScanOptions {
        cache_dir: None,
        ..Default::default()
    };
    sat_scan::scan_batched(&jobs, &opts, &sat_scan::ScanProgress::default(), 64, |i, o| {
        facts[i] = Some(o.facts);
    });
    let mut w_facts = Vec::new();
    for (c, f) in take.iter().zip(facts) {
        let folder = c.folder.clone().unwrap_or_else(|| c.name.clone());
        let (files, bytes) =
            sat_mo2::donors::count(&mods_dir.join(&folder)).map_err(|e| format!("{folder}: {e}"))?;
        let id = project.plan_install(
            &c.name,
            Source::Donor {
                donor: name.clone(),
                folder,
                files: Some(files),
                bytes: Some(bytes),
                moved,
            },
            None,
            after.as_deref(),
            c.enabled,
        )?;
        after = Some(c.name.clone());
        let sig = files.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ bytes;
        if let Some(f) = f {
            w_facts.push((id, sig, f));
        }
        println!(
            "planned: {} ← donor {name}/{}",
            c.name,
            c.folder.as_deref().unwrap_or(&c.name)
        );
    }
    let mut w = stll.begin().map_err(|e| e.to_string())?;
    for (id, sig, f) in &w_facts {
        w.put_facts(&id.0, *sig, f).map_err(|e| e.to_string())?;
    }
    w.finish(&project, sat_stll::file::keep_entities(&project))
        .map_err(|e| e.to_string())?;
    println!(
        "{} mod(s) planned from donor {name:?}{}; `sat-cli analyze` shows their conflicts, `sat-cli commit --apply` brings the folders",
        take.len(),
        if moved { " (to be moved out of it)" } else { "" }
    );
    Ok(())
}
