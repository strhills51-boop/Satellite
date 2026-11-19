//! The full commit plan of a build: sync plan, factories, patches, plugin files, project after. @sat:home.plan
//!
//! Owns: completing `@sat:sync.plan` with what only this machine knows — planned mods' sources here
//! (`@sat:home.plan.source`), dirty factories (`@sat:home.plan.factories`), the patches output
//! (`@sat:home.plan.patches`), what `plugins.txt` / `loadorder.txt` receive (`@sat:home.plan.plugins`) — and the
//! project as it will be after the commit (`project_after`). Every shell gets the same plan: what the window shows
//! is what the CLI runs (ADR 0019 §4).
//! Not here: the plan's rules (`@sat:sync.plan`), running it (`@sat:home.commit`, `@sat:mo2.commit`).

use sat_core::intent::Source;
use sat_core::project::Project;
use sat_core::sync::plan::{Blocker, DiskOp, Plan, plan_with};
use sat_mo2::commit::Layout;
use sat_mo2::{Instance, profile};

use crate::subfolders;

/// A commit plan with everything the commit writes besides it.
#[derive(Debug, Clone)]
pub struct FullPlan {
    pub plan: Plan,
    /// What `plugins.txt` / `loadorder.txt` receive; `None` — both stay.
    pub plugins: Option<PluginWrite>,
    /// Factories built: (index in the project, fingerprint of the inputs).
    pub built: Vec<(usize, String)>,
    /// Factories sealed (their groups leave the project with the members).
    pub sealed: Vec<usize>,
    /// The patches output: `Some(Some(fp))` written, `Some(None)` taken away, `None` untouched.
    pub patched: Option<Option<String>>,
    /// Dirty processors to run after the commit wrote the profile, in order: (index in the project, the run).
    pub processors: Vec<(usize, sat_mo2::processors::Run)>,
    /// MO2 of this build and its profile, to launch the processors' tools through (`None` — not found).
    pub mo2: Option<(std::path::PathBuf, String)>,
    pub overwrite_dir: std::path::PathBuf,
    /// What the shell should say about the plan (skipped factories, plugin lists changed on both sides).
    pub notes: Vec<String>,
}

// @sat:home.plan.full
/// The commit plan of the project against the mods folder now. `confirmed` names the steps the user confirmed
/// (sealing a group, `Blocker::NeedsConfirmation`).
pub fn full(inst: &Instance, layout: &Layout, project: &Project, confirmed: &[&str]) -> FullPlan {
    let disk = subfolders(&inst.mods_dir);
    let planned = project.planned();
    let mut plan = plan_with(&project.canon, &project.sync, &planned, &disk, &|s| {
        source_ok(inst, s)
    });
    let mut notes = Vec::new();
    let (built, sealed) = factories(project, &disk, &mut plan, confirmed, &mut notes);
    let patched = patches(project, layout, &disk, &mut plan);
    let processors = processors(inst, project, &disk, &mut plan, &mut notes);
    let plugins = plugin_files(layout, project, &plan, &mut notes);
    FullPlan {
        plan,
        plugins,
        built,
        sealed,
        patched,
        processors,
        mo2: sat_mo2::executables::mo2_exe(&inst.ini_path, &inst.base_dir).map(|e| (e, inst.profile.clone())),
        overwrite_dir: inst.overwrite_dir.clone(),
        notes,
    }
}

// @sat:home.plan.after
/// The project as the commit leaves it: the plan applied, the plugin lists as written, built factories recorded,
/// sealed groups gone, the patches fingerprint.
pub fn project_after(project: &Project, f: &FullPlan) -> Project {
    let mut after = project.clone();
    after.apply_plan(&f.plan);
    if let Some(w) = &f.plugins {
        // the files are now what the project says: the next analysis finds no edits of its own
        let mut seen = w.written.clone();
        seen.blacklist.clear();
        after.plugins = w.written.clone();
        after.plugins_seen = Some(seen);
    }
    for (i, fp) in &f.built {
        after.factories[*i].built = Some(fp.clone());
    }
    // a sealed group is done: its factory and the group itself leave the project with the members
    let gone: Vec<String> = f
        .sealed
        .iter()
        .map(|&i| project.factories[i].group.clone())
        .collect();
    after.decor.groups.retain(|g| !gone.contains(&g.id));
    let mut i = 0;
    after.factories.retain(|_| {
        i += 1;
        !f.sealed.contains(&(i - 1))
    });
    if let Some(fp) = &f.patched {
        after.patches_built = fp.clone();
    }
    after
}

// @sat:home.plan.processors
/// Dirty processors (`@sat:processor`), in their order (`assess`). A dirty processor's output mod is put at the end
/// of the canon (and its folder made) when absent, so MO2 lists it; the tool runs after the commit wrote the profile
/// (`@sat:home.commit.run`).
pub(crate) fn processors(
    inst: &Instance,
    project: &Project,
    disk: &[String],
    p: &mut Plan,
    notes: &mut Vec<String>,
) -> Vec<(usize, sat_mo2::processors::Run)> {
    use sat_core::canon::{Entry, EntryKind};
    use sat_core::processor::State;
    if !project.processors.iter().any(|x| x.enabled) {
        return Vec::new();
    }
    let tools = sat_mo2::executables::read(&inst.ini_path);
    let mut out = Vec::new();
    for (i, st, run) in assess(inst, project, &p.canon_after, disk, &tools) {
        let proc = &project.processors[i];
        match (&st.state, run) {
            (State::Cycle { with }, _) => {
                let n = format!("processors wait on each other ({}): none runs", with.join(" → "));
                if !notes.contains(&n) {
                    notes.push(n);
                }
            }
            (State::NoTool, _) => notes.push(format!(
                "processor {}: no tool «{}» in MO2",
                proc.title, proc.executable
            )),
            (State::Dirty { .. }, Some(run)) => {
                if !p
                    .canon_after
                    .entries
                    .iter()
                    .any(|e| e.name.eq_ignore_ascii_case(&proc.output))
                {
                    let mut entries = p.canon_after.entries.clone();
                    entries.push(Entry {
                        name: proc.output.clone(),
                        kind: EntryKind::Mod,
                        enabled: true,
                    });
                    p.canon_after = sat_core::canon::Modlist::new(entries);
                }
                let on_disk = disk.iter().any(|d| d.eq_ignore_ascii_case(&proc.output));
                if !on_disk
                    && !p
                        .disk_ops
                        .iter()
                        .any(|o| matches!(o, DiskOp::Create { name, .. } if name.eq_ignore_ascii_case(&proc.output)))
                {
                    p.disk_ops.push(DiskOp::Create {
                        name: proc.output.clone(),
                        kind: sat_core::sync::records::EntityKind::Mod,
                    });
                }
                out.push((i, run));
            }
            _ => {}
        }
    }
    out
}

// @sat:home.plan.processors.assess
/// Every processor of the project against the build, in run order (a cycle — all of them, each `Cycle`): inputs
/// are the enabled mods of `canon` that hold one of the selector's folders (or plugins at their root), with their
/// content signatures — other processors' outputs only when this one runs after them, never the mod the tool lives in; the tool is found in MO2's
/// list by its title and identified by its binary's size and time. A dirty one comes with its run.
pub fn assess(
    inst: &Instance,
    project: &Project,
    canon: &sat_core::canon::Modlist,
    disk: &[String],
    tools: &[sat_mo2::executables::Executable],
) -> Vec<(
    usize,
    sat_core::processor::Status,
    Option<sat_mo2::processors::Run>,
)> {
    use sat_core::canon::EntryKind;
    use sat_core::processor::{State, Status, dirty, fingerprint, order};
    let status = |p: &sat_core::processor::Processor, state, inputs, fingerprint| Status {
        id: p.id.clone(),
        state,
        inputs,
        fingerprint,
    };
    let ord = match order(&project.processors) {
        Ok(o) => o,
        Err(cycle) => {
            return project
                .processors
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    let st = State::Cycle { with: cycle.clone() };
                    (i, status(p, st, Vec::new(), String::new()), None)
                })
                .collect();
        }
    };
    let on_disk = |n: &str| disk.iter().any(|d| d.eq_ignore_ascii_case(n));
    let outputs: Vec<String> = project
        .processors
        .iter()
        .map(|x| x.output.to_lowercase())
        .collect();
    let mut out = Vec::new();
    for i in ord {
        let proc = &project.processors[i];
        let tool = tools
            .iter()
            .find(|t| t.title.eq_ignore_ascii_case(&proc.executable));
        let stamp = tool.and_then(|t| std::fs::metadata(&t.binary).ok()).map(|m| {
            let t = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs());
            (m.len(), t)
        });
        // the mod the tool itself lives in (the owner keeps generators in mods/) is no input: the tool writes its
        // caches and logs there, which would keep it dirty forever
        let home = tool.and_then(|t| {
            let rel = t.binary.strip_prefix(&inst.mods_dir).ok()?;
            rel.components()
                .next()
                .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
        });
        // its own output and the other processors' outputs it does not depend on are no inputs
        let inputs: Vec<(String, Option<u64>)> = canon
            .entries
            .iter()
            .filter(|e| e.kind == EntryKind::Mod && e.enabled)
            .filter(|e| home.as_deref() != Some(e.name.to_lowercase().as_str()))
            .filter(|e| {
                let n = e.name.to_lowercase();
                !outputs.contains(&n)
                    || proc.after.iter().any(|a| {
                        project
                            .processors
                            .iter()
                            .any(|x| &x.id == a && x.output.eq_ignore_ascii_case(&e.name))
                    })
            })
            .filter(|e| {
                let dir = inst.mods_dir.join(&e.name);
                let plugins = proc.inputs.plugins
                    && std::fs::read_dir(&dir).is_ok_and(|rd| {
                        rd.flatten().any(|f| {
                            let n = f.file_name().to_string_lossy().to_lowercase();
                            n.ends_with(".esp") || n.ends_with(".esm") || n.ends_with(".esl")
                        })
                    });
                proc.inputs.selects(|f| dir.join(f).is_dir(), plugins)
            })
            .map(|e| {
                let sig = project
                    .entities
                    .id_of(&e.name)
                    .and_then(|id| project.signatures.get(&id.0))
                    .copied();
                (e.name.clone(), sig)
            })
            .collect();
        let fp = fingerprint(proc, &inputs, stamp);
        let names = inputs.into_iter().map(|(n, _)| n).collect();
        let (state, run) = if !proc.enabled {
            (State::Off, None)
        } else if let Some(tool) = tool {
            match dirty(proc, &fp, on_disk(&proc.output)) {
                None => (State::Clean, None),
                Some(why) => (
                    State::Dirty { why },
                    Some(sat_mo2::processors::Run {
                        id: proc.id.clone(),
                        executable: tool.title.clone(),
                        output: proc.output.clone(),
                        fingerprint: fp.clone(),
                    }),
                ),
            }
        } else {
            (State::NoTool, None)
        };
        out.push((i, status(proc, state, names, fp), run));
    }
    out
}

// @sat:home.plan.processors.status
/// The processors of `project` as the window shows them, with the tools MO2 has registered on this machine
/// (title, binary file name) — for the presets to recognise.
pub fn processor_status(
    inst: &Instance,
    project: &Project,
) -> (Vec<sat_core::processor::Status>, Vec<(String, String)>) {
    let tools = sat_mo2::executables::read(&inst.ini_path);
    let disk = subfolders(&inst.mods_dir);
    let mut st: Vec<(usize, sat_core::processor::Status)> =
        assess(inst, project, &project.canon, &disk, &tools)
            .into_iter()
            .map(|(i, s, _)| (i, s))
            .collect();
    st.sort_by_key(|(i, _)| *i);
    (
        st.into_iter().map(|(_, s)| s).collect(),
        tools.iter().map(|t| (t.title.clone(), t.file_name())).collect(),
    )
}

// @sat:home.plan.plugins
/// What a commit writes to `plugins.txt` / `loadorder.txt`.
#[derive(Debug, Clone)]
pub struct PluginWrite {
    pub plugins: Vec<u8>,
    pub loadorder: Vec<u8>,
    /// The lists as written: the project's canon and its `plugins_seen` after the commit.
    pub written: sat_core::plugins::PluginCanon,
    /// The project's own edits are in it (order, marks, hidden plugins).
    pub edited: bool,
    /// Plugins of packed factory outputs, added enabled at the end.
    pub packed: Vec<String>,
}

/// The plugin files a commit writes, or `None` when both stay as they are: the project's plugin canon when it
/// differs from the manager's files (`@sat:plugins.edit.canon-files`), plus the plugins of packed outputs. The
/// canon is reconciled with the files first (`@sat:plugins.edit.reconcile`): a change made in the manager after
/// the last analysis is not overwritten by the project's older lists.
pub(crate) fn plugin_files(
    layout: &Layout,
    project: &Project,
    p: &sat_core::sync::plan::Plan,
    notes: &mut Vec<String>,
) -> Option<PluginWrite> {
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
        checked: profile::parse_plugins(&pt, sat_core::game::active().plugins.stars)
            .into_iter()
            .map(|l| (l.name, l.enabled))
            .collect(),
        blacklist: Vec::new(),
    };
    let (canon, r) = sat_core::plugin_edit::reconcile(&project.plugins, project.plugins_seen.as_ref(), &disk);
    if r.conflict {
        notes.push(
            "plugins: the manager changed the same lists since the last analysis — the project's edit is written"
                .into(),
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
        plugins: profile::serialize_plugins(&lines, &pt, sat_core::game::active().plugins.stars),
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

// @sat:home.plan.patches
/// Adds the patches output (`@sat:patch`) to the plan when the patches changed or it is not on disk: bases are
/// checked (a changed base blocks), the old output goes to `deleted/`, the output is placed last and enabled.
/// Returns the fingerprint to record (`Some(None)` — the output was taken away), `None` — nothing to do.
pub(crate) fn patches(
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

// @sat:home.plan.factories
/// Adds the work of dirty factories to the plan (`@sat:factory`): an output whose inputs changed (or that is not
/// on disk) is rebuilt — the old output goes to `deleted/` — and placed after its group. Returns (factory index,
/// fingerprint) of the ones built, recorded in the project after the commit.
pub(crate) fn factories(
    project: &Project,
    disk: &[String],
    p: &mut sat_core::sync::plan::Plan,
    confirmed: &[&str],
    notes: &mut Vec<String>,
) -> (Vec<(usize, String)>, Vec<usize>) {
    use sat_core::factory::{FactoryMode, fingerprint, place_output};
    let mut sealed = Vec::new();
    let groups = project.book().0.decor().groups;
    let on_disk = |n: &str| disk.iter().any(|d| d.eq_ignore_ascii_case(n));
    let mut out = Vec::new();
    for (i, f) in project.factories.iter().enumerate() {
        let Some(g) = groups.iter().find(|g| g.id == f.group) else {
            notes.push(format!("factory {}: its group is gone — skipped", f.output));
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

// @sat:home.plan.source
/// Whether a source is here and is what the intent saw (a different file of the same name is not taken).
pub fn source_ok(inst: &Instance, s: &Source) -> Result<(), String> {
    match s {
        Source::Downloads { file, size } => {
            let path = inst.downloads_dir.join(file);
            let md =
                std::fs::metadata(&path).map_err(|_| format!("{} is not in the downloads folder", file))?;
            match size {
                Some(n) if *n != md.len() => Err(format!(
                    "downloads/{file} is {} bytes, the intent saw {n} — another file of that name",
                    md.len()
                )),
                _ => Ok(()),
            }
        }
        Source::Donor {
            donor,
            folder,
            files,
            bytes,
            ..
        } => {
            let root = sat_mo2::donors::load(&inst.profile_dir())
                .remove(donor)
                .ok_or_else(|| {
                    format!("donor {donor:?} is not bound on this machine (sat-cli donor … --as {donor:?})")
                })?;
            let path = root.join(folder);
            let (f, b) =
                sat_mo2::donors::count(&path).map_err(|_| format!("{} is not there", path.display()))?;
            match (files, bytes) {
                (Some(nf), Some(nb)) if (*nf, *nb) != (f, b) => Err(format!(
                    "{} has {f} files / {b} bytes, the intent saw {nf} / {nb} — it changed since",
                    path.display()
                )),
                _ => Ok(()),
            }
        }
    }
}
