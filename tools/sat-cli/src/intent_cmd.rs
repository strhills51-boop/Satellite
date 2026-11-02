//! `sat-cli plan-install`: a planned mod — its place in the order now, its files at the next commit. @sat:cli.intent
//!
//! Owns: turning an archive of the downloads folder into a planned mod of the project (canon entry, install
//! intent, the facts its archive lists), and checking a source on this machine before a commit.
//! Not here: intents (`@sat:intent`), installing (`@sat:mo2.install`), the commit (`@sat:cli.commit`).

use std::path::Path;

use sat_core::intent::Source;
use sat_mo2::Instance;
use sat_mo2::commit::Layout;

use crate::{flag_value, has, open_instance, project_io};

/// Facts signature of an archive's listing: changes when the archive does.
fn listing_signature(file: &str, size: u64) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in file.to_lowercase().bytes().chain(size.to_le_bytes()) {
        h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// `sat-cli plan-install <instance> --archive FILE [--name NAME] [--after MOD] [--disabled] [--choose
/// "step/group/option"]... [--show]`: FILE is a file of the downloads folder (by name, or a path inside it). A FOMOD
/// archive takes the installer's defaults unless options are chosen; `--show` lists the steps and changes nothing.
/// Nothing on disk changes until `sat-cli commit --apply`.
pub fn plan_install(args: &[String]) -> Result<(), String> {
    let inst = open_instance(args)?;
    let layout = Layout::of(&inst);
    let arg = flag_value(args, "--archive").ok_or("--archive FILE: an archive of the downloads folder")?;
    let file = Path::new(arg)
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .ok_or("--archive: a file name")?;
    let path = inst.downloads_dir.join(&file);
    if !path.is_file() {
        return Err(format!(
            "{}: not in the downloads folder — the project refers to downloads by name, so put the archive there",
            path.display()
        ));
    }
    let size = std::fs::metadata(&path).map_err(|e| e.to_string())?.len();
    let given = crate::flag_values(args, "--choose")
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    let archive = sat_mo2::install::inspect_with(&path, (!given.is_empty()).then_some(given.as_slice()))?;
    if let Some(f) = &archive.fomod {
        show_fomod(f);
        if has(args, "--show") {
            return Ok(());
        }
    } else if has(args, "--show") {
        println!(
            "{file}: no installer; root {:?}, {} files",
            archive.root,
            archive.files.len()
        );
        return Ok(());
    }
    let stem = file.rsplit_once('.').map_or(file.as_str(), |x| x.0).to_string();
    let name = flag_value(args, "--name").map_or(stem, str::to_string);
    let (mut stll, mut project) = project_io::require(&layout.project_file)?;
    let id = project.plan_install(
        &name,
        Source::Downloads {
            file: file.clone(),
            size: Some(size),
        },
        archive.fomod.as_ref().map(|f| f.chosen.clone()),
        flag_value(args, "--after"),
        !has(args, "--disabled"),
    )?;
    let facts = sat_mo2::install::facts_of(&name, &archive);
    let mut w = stll.begin().map_err(|e| e.to_string())?;
    w.put_facts(&id.0, listing_signature(&file, size), &facts)
        .map_err(|e| e.to_string())?;
    w.finish(&project, sat_stll::file::keep_entities(&project))
        .map_err(|e| e.to_string())?;
    println!(
        "planned: {name} ← downloads/{file} ({} files, root {:?}); run `sat-cli analyze` to see its conflicts, `sat-cli commit --apply` to install",
        archive.files.len(),
        if archive.root.is_empty() {
            "/"
        } else {
            archive.root.as_str()
        }
    );
    Ok(())
}

/// Prints a FOMOD installer: visible steps, groups and options, the ones in effect marked.
fn show_fomod(f: &sat_mo2::install::FomodChoice) {
    use sat_formats::fomod::choice_id;
    println!("FOMOD installer: {}", f.module);
    let on: std::collections::HashSet<String> = f.chosen.iter().map(|c| c.to_lowercase()).collect();
    for s in &f.config.steps {
        println!("  step {:?}", s.name);
        for g in &s.groups {
            println!("    group {:?} ({:?})", g.name, g.kind);
            for o in &g.options {
                let id = choice_id(&s.name, &g.name, &o.name);
                let mark = if on.contains(&id.to_lowercase()) {
                    "[x]"
                } else {
                    "[ ]"
                };
                println!("      {mark} {id}   ({:?})", o.kind);
            }
        }
    }
    for m in &f.missing {
        println!("  note: the config names {m:?}, the archive does not have it (skipped)");
    }
    println!("  choose with --choose \"step/group/option\" (repeat); a group not named keeps its defaults");
}

// @sat:cli.intent.source
/// Whether a source is here and is what the intent saw (a different file of the same name is not taken).
pub fn source_ok(inst: &Instance, s: &Source) -> Result<(), String> {
    let Source::Downloads { file, size } = s;
    let path = inst.downloads_dir.join(file);
    let md = std::fs::metadata(&path).map_err(|_| format!("{} is not in the downloads folder", file))?;
    match size {
        Some(n) if *n != md.len() => Err(format!(
            "downloads/{file} is {} bytes, the intent saw {n} — another file of that name",
            md.len()
        )),
        _ => Ok(()),
    }
}
