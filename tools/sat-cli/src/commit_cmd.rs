//! `sat-cli sync` and `sat-cli commit`: decide sync records, then commit them to the build. @sat:cli.commit
//!
//! Owns: the command line around the commit pipeline — listing the records of the last analysis, recording a
//! decision in the project, printing the plan, and running it only when asked (`--apply`).
//! Not here: the plan (`@sat:sync.plan`), running it (`@sat:mo2.commit`), finding the records (`analyze`).
//!
//! The pipeline of the owner's philosophy: scan/sync (`analyze`) → process (decide records, edit the canon) →
//! commit (this). Without `--apply` nothing on disk changes; only the project file receives decisions.

use std::path::PathBuf;

use sat_core::project::Project;
use sat_core::sync::plan::{Blocker, DiskOp, ProjectOp, plan_with};
use sat_core::sync::records::{Mismatch, Resolution, Status, SyncRecord};
use sat_mo2::commit::{Faults, Layout, Outcome, PROJECT_BEFORE, Payload, execute, stamp_now};
use sat_mo2::profile;

use crate::{flag_value, has, open_instance, project_io, subfolders};

fn load(args: &[String]) -> Result<(sat_mo2::Instance, Layout, sat_stll::Stll, Project), String> {
    let inst = open_instance(args)?;
    let layout = Layout::of(&inst);
    let (stll, project) = project_io::require(&layout.project_file)?;
    Ok((inst, layout, stll, project))
}

fn describe(r: &SyncRecord) -> String {
    let what = match &r.mismatch {
        Mismatch::FsCreated { fs_name } => format!("new folder        {fs_name}"),
        Mismatch::FsDeleted { project_name } => format!("folder missing    {project_name}"),
        Mismatch::FsRenamed {
            project_name,
            fs_name,
        } => format!("folder renamed    {project_name} → {fs_name}"),
    };
    let status = match r.status {
        Status::Pending => "pending".to_string(),
        Status::Ignored => "ignored".to_string(),
        Status::Confirmed => format!(
            "confirmed: {}",
            r.resolution.map_or("—".into(), |x| serde_json::to_string(&x)
                .unwrap_or_default()
                .replace('"', ""))
        ),
    };
    format!("{what}   [{status}]")
}

fn answers(m: &Mismatch) -> &'static str {
    match m {
        Mismatch::FsCreated { .. } => "adopt_to_project | move_to_deleted",
        Mismatch::FsDeleted { .. } => "create_disk_folder | remove_from_project",
        Mismatch::FsRenamed { .. } => "accept_fs_name | rename_disk_to_project",
    }
}

// @sat:cli.commit.sync
/// `sat-cli sync <instance> [--decide N ANSWER]...`: lists the records; `--decide` records a decision in the project.
/// ANSWER is a resolution (`adopt_to_project`, …), `ignore` or `pending`.
pub fn sync(args: &[String]) -> Result<(), String> {
    let (_, layout, mut stll, mut project) = load(args)?;
    let decisions: Vec<(&str, &str)> = args
        .windows(3)
        .filter(|w| w[0] == "--decide")
        .map(|w| (w[1].as_str(), w[2].as_str()))
        .collect();
    for (n, answer) in &decisions {
        let i: usize = n
            .parse()
            .map_err(|_| format!("--decide {n}: a record number from the list"))?;
        let r = project
            .sync
            .get_mut(i.wrapping_sub(1))
            .ok_or_else(|| format!("--decide {n}: no record {n}"))?;
        match *answer {
            "ignore" => {
                r.status = Status::Ignored;
                r.resolution = None;
            }
            "pending" => {
                r.status = Status::Pending;
                r.resolution = None;
            }
            a => {
                let res: Resolution = serde_json::from_str(&format!("\"{a}\"")).map_err(|_| {
                    format!(
                        "--decide {n} {a}: answers for this record: {}",
                        answers(&r.mismatch)
                    )
                })?;
                if !res.fits(&r.mismatch) {
                    return Err(format!(
                        "--decide {n} {a}: answers for this record: {}",
                        answers(&r.mismatch)
                    ));
                }
                r.status = Status::Confirmed;
                r.resolution = Some(res);
            }
        }
    }
    if !decisions.is_empty() {
        project_io::save(&mut stll, &project)?;
        eprintln!("decisions saved to {}", layout.project_file.display());
    }
    if project.sync.is_empty() {
        println!("no sync records: the project and the mods folder agree (as of the last analysis)");
    }
    for (i, r) in project.sync.iter().enumerate() {
        println!("{:>3}. {}", i + 1, describe(r));
        if r.status == Status::Pending {
            println!("       answers: {} | ignore", answers(&r.mismatch));
        }
    }
    Ok(())
}

// @sat:cli.commit.run
/// `sat-cli commit <instance> [--apply]`: prints the plan; with `--apply` runs it (backup, journal, rollback).
pub fn commit(args: &[String]) -> Result<(), String> {
    let (inst, layout, mut stll, project) = load(args)?;
    let disk = subfolders(&inst.mods_dir);
    let planned = project.planned();
    let mut p = plan_with(&project.canon, &project.sync, &planned, &disk, &|s| {
        crate::intent_cmd::source_ok(&inst, s)
    });
    let confirmed: Vec<&str> = crate::flag_values(args, "--confirm");
    let (built, sealed) = factories(&project, &disk, &mut p, &confirmed);
    let patched = patches(&project, &layout, &disk, &mut p);
    println!("commit plan for {}:", inst.profile_dir().display());
    for op in &p.project_ops {
        match op {
            ProjectOp::Adopt { name, .. } => println!("  project: add {name} (at the end, enabled)"),
            ProjectOp::Remove { name } => println!("  project: remove {name}"),
            ProjectOp::Rename { from, to } => println!("  project: rename {from} → {to}"),
        }
    }
    for op in &p.disk_ops {
        match op {
            DiskOp::ToDeleted { name } => println!("  disk: move {name} → satellite/deleted/<stamp>/"),
            DiskOp::Rename { from, to } => println!("  disk: rename folder {from} → {to}"),
            DiskOp::Create { name, .. } => println!("  disk: create empty folder {name}"),
            DiskOp::Patches { name, patches } => {
                println!("  disk: write {name} ({} patched file(s))", patches.len())
            }
            DiskOp::Bake {
                name,
                members,
                link,
                pack,
                downscale,
            } => println!(
                "  disk: build {name} from {} member(s){}{}{} — the members are switched off unless kept",
                members.len(),
                if *downscale > 0 {
                    format!(", textures smaller by {downscale} mip level(s)")
                } else {
                    String::new()
                },
                if *link { " (hard links)" } else { "" },
                if *pack {
                    " packed into archives with an empty light plugin (enabled in plugins.txt)"
                } else {
                    ""
                }
            ),
            DiskOp::Install { name, source, fomod } => {
                println!("  disk: install {name} from {}", source.describe());
                for c in fomod.iter().flatten() {
                    println!("        FOMOD: {c}");
                }
            }
        }
    }
    let plugins = plugin_files(&layout, &project, &p);
    if let Some(w) = &plugins {
        println!(
            "  plugins.txt / loadorder.txt: {}{}",
            if w.edited {
                "written from the project (its order, marks and hidden plugins)"
            } else {
                "kept"
            },
            if w.packed.is_empty() {
                String::new()
            } else {
                format!(", plus {} enabled at the end", w.packed.join(", "))
            }
        );
    }
    println!(
        "  modlist.txt: written from the project canon ({} entries); {} record(s) settled, {} ignored",
        p.canon_after.entries.len(),
        p.settles.len(),
        p.ignored
    );
    if p.blocked() {
        println!("blocked:");
        for b in &p.blockers {
            println!("  {}", blocker(b, &project.sync));
        }
        return Err("the plan is blocked; nothing was changed".into());
    }
    if !has(args, "--apply") {
        println!("dry run: nothing was changed (add --apply to commit)");
        return Ok(());
    }
    let style = std::fs::read(layout.profile_dir.join("modlist.txt"))
        .map(|d| profile::parse_modlist(&d).1)
        .unwrap_or_default();
    let mut after = project.clone();
    after.apply_plan(&p);
    if let Some(w) = &plugins {
        // the files are now what the project says: the next analysis finds no edits of its own
        let mut seen = w.written.clone();
        seen.blacklist.clear();
        after.plugins = w.written.clone();
        after.plugins_seen = Some(seen);
    }
    for (i, fp) in built {
        after.factories[i].built = Some(fp);
    }
    // a sealed group is done: its factory and the group itself leave the project with the members
    let gone: Vec<String> = sealed
        .iter()
        .map(|&i| project.factories[i].group.clone())
        .collect();
    after.decor.groups.retain(|g| !gone.contains(&g.id));
    let mut i = 0;
    after.factories.retain(|_| {
        i += 1;
        !sealed.contains(&(i - 1))
    });
    if let Some(fp) = patched {
        after.patches_built = fp;
    }
    // the new project file is prepared beside the old one; the commit swaps them inside its transaction
    let prepared = layout.project_file.with_extension("stll-next");
    stll.begin()
        .and_then(|w| w.finish_to(&after, sat_stll::file::keep_entities(&after), &prepared))
        .map_err(|e| e.to_string())?;
    drop(stll);
    let payload = Payload {
        plan: &p,
        modlist: profile::serialize_modlist(&p.canon_after, &style),
        project_after: prepared.clone(),
        plugin_files: plugins.map(|w| (w.plugins, w.loadorder)),
    };
    let stamp = flag_value(args, "--stamp").map_or_else(stamp_now, str::to_string);
    let report = execute(&layout, &stamp, &payload, Faults::default());
    println!("version: {}", report.version_dir.display());
    if prepared.exists() {
        let _ = std::fs::remove_file(&prepared);
    }
    match report.outcome {
        Outcome::Committed => {
            // the version keeps the user's truth; the facts are rebuildable and stay in the live project only
            let before = report.version_dir.join(PROJECT_BEFORE);
            if before.exists()
                && let Err(e) = sat_stll::strip_facts(&before)
            {
                eprintln!("{}: facts not stripped: {e}", before.display());
            }
            println!("committed: {} step(s)", report.steps.len());
            Ok(())
        }
        Outcome::RolledBack { error } => Err(format!("failed and rolled back: {error}")),
        Outcome::RollbackIncomplete { error, left } => Err(format!(
            "failed: {error}; NOT undone: {} — see {}",
            left.join("; "),
            PathBuf::from(&report.version_dir)
                .join("commit.journal.json")
                .display()
        )),
        Outcome::Refused { error } => Err(format!("refused: {error}")),
    }
}

// @sat:cli.commit.plugins
/// What a commit writes to `plugins.txt` / `loadorder.txt`.
struct PluginWrite {
    plugins: Vec<u8>,
    loadorder: Vec<u8>,
    /// The lists as written: the project's canon and its `plugins_seen` after the commit.
    written: sat_core::plugins::PluginCanon,
    /// The project's own edits are in it (order, marks, hidden plugins).
    edited: bool,
    /// Plugins of packed factory outputs, added enabled at the end.
    packed: Vec<String>,
}

/// The plugin files a commit writes, or `None` when both stay as they are: the project's plugin canon when it
/// differs from the manager's files (`@sat:plugins.edit.canon-files`), plus the plugins of packed outputs. The
/// canon is reconciled with the files first (`@sat:plugins.edit.reconcile`): a change made in the manager after
/// the last analysis is not overwritten by the project's older lists.
fn plugin_files(layout: &Layout, project: &Project, p: &sat_core::sync::plan::Plan) -> Option<PluginWrite> {
    use sat_core::plugin_edit::canon_files;
    let packed: Vec<String> = p
        .disk_ops
        .iter()
        .filter_map(|op| match op {
            DiskOp::Bake { name, pack: true, .. } => Some(sat_core::factory::pack_plugin(name)),
            _ => None,
        })
        .collect();
    let read = |f: &str| std::fs::read(layout.profile_dir.join(f)).unwrap_or_default();
    let (pt, lo) = (read("plugins.txt"), read("loadorder.txt"));
    let disk = sat_core::plugins::PluginCanon {
        load_order: profile::parse_loadorder(&lo),
        checked: profile::parse_plugins(&pt)
            .into_iter()
            .map(|l| (l.name, l.enabled))
            .collect(),
        blacklist: Vec::new(),
    };
    let (canon, r) = sat_core::plugin_edit::reconcile(&project.plugins, project.plugins_seen.as_ref(), &disk);
    if r.conflict {
        eprintln!(
            "plugins: the manager changed the same lists since the last analysis — the project's edit is written"
        );
    }
    let canon = &canon;
    let (mut lines, mut order) = canon_files(canon);
    let edited = (lines.clone(), order.clone()) != canon_files(&disk);
    if !edited && packed.is_empty() {
        return None;
    }
    if !edited {
        // only the packed plugins: the files keep their own lines
        lines = disk.checked.clone();
        order = disk.load_order.clone();
    }
    for e in &packed {
        match lines.iter_mut().find(|(n, _)| n.eq_ignore_ascii_case(e)) {
            Some(l) => l.1 = true,
            None => lines.push((e.clone(), true)),
        }
        if !order.iter().any(|n| n.eq_ignore_ascii_case(e)) {
            order.push(e.clone());
        }
    }
    Some(PluginWrite {
        plugins: profile::serialize_plugins(&lines, &pt),
        loadorder: profile::serialize_loadorder(&order, &lo),
        written: sat_core::plugins::PluginCanon {
            load_order: order,
            checked: lines,
            blacklist: canon.blacklist.clone(),
        },
        edited,
        packed,
    })
}

// @sat:cli.commit.patches
/// Adds the patches output (`@sat:patch`) to the plan when the patches changed or it is not on disk: bases are
/// checked (a changed base blocks), the old output goes to `deleted/`, the output is placed last and enabled.
/// Returns the fingerprint to record (`Some(None)` — the output was taken away), `None` — nothing to do.
fn patches(
    project: &Project,
    layout: &Layout,
    disk: &[String],
    p: &mut sat_core::sync::plan::Plan,
) -> Option<Option<String>> {
    use sat_core::canon::{Entry, EntryKind};
    use sat_core::patch::{PATCH_MOD, fingerprint};
    let on_disk = disk.iter().any(|d| d.eq_ignore_ascii_case(PATCH_MOD));
    let mut entries: Vec<Entry> = p
        .canon_after
        .entries
        .iter()
        .filter(|e| !e.name.eq_ignore_ascii_case(PATCH_MOD))
        .cloned()
        .collect();
    if project.patches.is_empty() {
        if !on_disk && project.patches_built.is_none() {
            return None;
        }
        if on_disk {
            p.disk_ops.push(DiskOp::ToDeleted {
                name: PATCH_MOD.into(),
            });
        }
        p.canon_after = sat_core::canon::Modlist::new(entries);
        return Some(None);
    }
    let by_name: Vec<sat_core::patch::FilePatch> = project
        .patches
        .iter()
        .map(|x| sat_core::patch::FilePatch {
            source: project
                .entities
                .name_of(&sat_core::entity::EntityId(x.source.clone()))
                .unwrap_or(&x.source)
                .to_string(),
            ..x.clone()
        })
        .collect();
    let fp = fingerprint(&by_name);
    if project.patches_built.as_deref() == Some(fp.as_str()) && on_disk {
        return None;
    }
    for x in &by_name {
        match sat_mo2::patches::base_holds(&layout.mods_dir, x) {
            Ok(true) => {}
            Ok(false) => p.blockers.push(Blocker::PatchBaseChanged {
                source: x.source.clone(),
                path: x.path.clone(),
            }),
            Err(_) => p.blockers.push(Blocker::MissingFolder {
                name: format!("{}/{}", x.source, x.path),
            }),
        }
    }
    if on_disk {
        p.disk_ops.push(DiskOp::ToDeleted {
            name: PATCH_MOD.into(),
        });
    }
    p.disk_ops.push(DiskOp::Patches {
        name: PATCH_MOD.into(),
        patches: by_name,
    });
    entries.push(Entry {
        name: PATCH_MOD.into(),
        kind: EntryKind::Mod,
        enabled: true,
    });
    p.canon_after = sat_core::canon::Modlist::new(entries);
    Some(Some(fp))
}

// @sat:cli.commit.factories
/// Adds the work of dirty factories to the plan (`@sat:factory`): an output whose inputs changed (or that is not
/// on disk) is rebuilt — the old output goes to `deleted/` — and placed after its group. Returns (factory index,
/// fingerprint) of the ones built, recorded in the project after the commit.
fn factories(
    project: &Project,
    disk: &[String],
    p: &mut sat_core::sync::plan::Plan,
    confirmed: &[&str],
) -> (Vec<(usize, String)>, Vec<usize>) {
    use sat_core::factory::{FactoryMode, fingerprint, place_output};
    let mut sealed = Vec::new();
    let groups = project.book().0.decor().groups;
    let on_disk = |n: &str| disk.iter().any(|d| d.eq_ignore_ascii_case(n));
    let mut out = Vec::new();
    for (i, f) in project.factories.iter().enumerate() {
        let Some(g) = groups.iter().find(|g| g.id == f.group) else {
            println!("  factory {}: its group is gone — skipped", f.output);
            continue;
        };
        let sigs: Vec<(String, Option<u64>)> = g
            .members
            .iter()
            .map(|m| {
                let sig = project
                    .entities
                    .id_of(m)
                    .and_then(|id| project.signatures.get(&id.0))
                    .copied();
                (m.clone(), sig)
            })
            .collect();
        let fp = fingerprint(f, &sigs);
        let seal = f.mode == FactoryMode::Seal;
        if !seal && f.built.as_deref() == Some(fp.as_str()) && on_disk(&f.output) {
            continue;
        }
        if seal && !confirmed.iter().any(|c| c.eq_ignore_ascii_case(&f.output)) {
            p.blockers.push(Blocker::NeedsConfirmation {
                what: format!(
                    "sealing {} takes its {} member(s) out of the build for good",
                    g.name,
                    g.members.len()
                ),
                confirm: f.output.clone(),
            });
        }
        for m in g.members.iter().filter(|m| !on_disk(m)) {
            p.blockers.push(Blocker::MissingFolder { name: m.clone() });
        }
        if on_disk(&f.output) {
            p.disk_ops.push(DiskOp::ToDeleted {
                name: f.output.clone(),
            });
        }
        p.disk_ops.push(DiskOp::Bake {
            name: f.output.clone(),
            members: g.members.clone(),
            link: f.link,
            pack: f.pack,
            downscale: f.downscale,
        });
        p.canon_after = place_output(&p.canon_after, f, &g.members);
        if seal {
            for m in &g.members {
                p.disk_ops.push(DiskOp::ToDeleted { name: m.clone() });
            }
            let members = &g.members;
            p.canon_after
                .entries
                .retain(|e| !members.iter().any(|m| m.eq_ignore_ascii_case(&e.name)));
            sealed.push(i);
        }
        out.push((i, fp));
    }
    (out, sealed)
}

fn blocker(b: &Blocker, records: &[SyncRecord]) -> String {
    let rec = |fp: &str| {
        records
            .iter()
            .position(|r| r.fingerprint == fp)
            .map_or(fp.to_string(), |i| {
                format!("record {} ({})", i + 1, describe(&records[i]))
            })
    };
    match b {
        Blocker::Pending { fingerprint } => format!(
            "{} waits for a decision (sat-cli sync --decide)",
            rec(fingerprint)
        ),
        Blocker::NoResolution { fingerprint } => {
            format!("{} is confirmed without an answer", rec(fingerprint))
        }
        Blocker::WrongResolution { fingerprint, .. } => {
            format!("{}: the answer does not fit", rec(fingerprint))
        }
        Blocker::Stale { fingerprint } => format!(
            "{}: the disk changed since the analysis — run analyze",
            rec(fingerprint)
        ),
        Blocker::MissingFolder { name } => format!("folder {name} is not there"),
        Blocker::TargetExists { name } => format!("folder {name} already exists"),
        Blocker::TargetTwice { name } => format!("two operations produce {name}"),
        Blocker::RenameCycle { names } => format!("renames wait on each other: {}", names.join(", ")),
        Blocker::CanonMismatch { name } => format!("{name}: the project list does not fit the change"),
        Blocker::SeparatorNotEmpty { name } => format!("separator {name} still has mods in its block"),
        Blocker::SourceUnavailable { name, why } => format!("planned mod {name}: {why}"),
        Blocker::NeedsConfirmation { what, confirm } => format!(
            "{what} — not undone by a later commit; to go ahead: sat-cli commit … --apply --confirm \"{confirm}\""
        ),
        Blocker::PatchBaseChanged { source, path } => format!(
            "{source}/{path} changed since it was patched (updated?): look at the patch again (sat-cli patch … --rebase)"
        ),
    }
}
