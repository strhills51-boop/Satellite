//! `sat-cli patch`: edits of a mod's ini file kept in the project, written at commit. @sat:cli.patch
//!
//! Owns: adding, changing and dropping file patches from the command line, recording the base the patch is made
//! on, rebasing after the mod was updated, and listing them.
//! Not here: patches as data (`@sat:patch`), applying (`@sat:mo2.patches`).

use sat_core::patch::{FilePatch, PatchOp, content_signature};
use sat_mo2::commit::Layout;

use crate::{flag_value, flag_values, has, open_instance, project_io};

fn section_key(s: &str) -> (String, String) {
    match s.rsplit_once('/') {
        Some((sec, key)) => (sec.trim().to_string(), key.trim().to_string()),
        None => (String::new(), s.trim().to_string()),
    }
}

/// `sat-cli patch <instance> [--mod MOD --file PATH [--set "SECTION/KEY=VALUE"]... [--unset "SECTION/KEY"]...
/// [--rebase] [--drop]]`: without `--mod` lists the patches.
pub fn patch(args: &[String]) -> Result<(), String> {
    let inst = open_instance(args)?;
    let layout = Layout::of(&inst);
    let (mut stll, mut project) = project_io::require(&layout.project_file)?;
    if let Some(m) = flag_value(args, "--mod") {
        let file = flag_value(args, "--file").ok_or("--mod needs --file PATH (inside the mod)")?;
        let path = file.replace('\\', "/").trim_matches('/').to_string();
        let id = project
            .entities
            .id_of(m)
            .ok_or_else(|| format!("no mod {m:?} in the project"))?
            .0
            .clone();
        let real = inst.mod_dir(
            project
                .entities
                .name_of(&sat_core::entity::EntityId(id.clone()))
                .unwrap_or(m),
        );
        let bytes =
            std::fs::read(real.join(&path)).map_err(|e| format!("{}: {e}", real.join(&path).display()))?;
        let at = project
            .patches
            .iter()
            .position(|x| x.source == id && x.path.eq_ignore_ascii_case(&path));
        if has(args, "--drop") {
            if let Some(i) = at {
                project.patches.remove(i);
            }
        } else {
            let i = at.unwrap_or_else(|| {
                project.patches.push(FilePatch {
                    source: id.clone(),
                    path: path.clone(),
                    base: Some(content_signature(&bytes)),
                    ops: Vec::new(),
                });
                project.patches.len() - 1
            });
            let p = &mut project.patches[i];
            if has(args, "--rebase") {
                p.base = Some(content_signature(&bytes));
            }
            let mut put = |op: PatchOp| {
                let same = |o: &PatchOp| match (o, &op) {
                    (
                        PatchOp::Set {
                            section: a, key: b, ..
                        }
                        | PatchOp::Remove { section: a, key: b },
                        PatchOp::Set {
                            section: c, key: d, ..
                        }
                        | PatchOp::Remove { section: c, key: d },
                    ) => a.eq_ignore_ascii_case(c) && b.eq_ignore_ascii_case(d),
                };
                p.ops.retain(|o| !same(o));
                p.ops.push(op);
            };
            for s in flag_values(args, "--set") {
                let (sk, value) = s
                    .split_once('=')
                    .ok_or_else(|| format!("--set {s:?}: SECTION/KEY=VALUE"))?;
                let (section, key) = section_key(sk);
                put(PatchOp::Set {
                    section,
                    key,
                    value: value.trim().to_string(),
                });
            }
            for s in flag_values(args, "--unset") {
                let (section, key) = section_key(s);
                put(PatchOp::Remove { section, key });
            }
        }
        project_io::save(&mut stll, &project)?;
        eprintln!("saved — `sat-cli commit` writes {}", sat_core::patch::PATCH_MOD);
    }
    if project.patches.is_empty() {
        println!("no patches (add one: --mod MOD --file PATH --set \"SECTION/KEY=VALUE\")");
    }
    for p in &project.patches {
        let name = project
            .entities
            .name_of(&sat_core::entity::EntityId(p.source.clone()))
            .unwrap_or(&p.source);
        let state = match std::fs::read(inst.mod_dir(name).join(&p.path)) {
            Ok(b) if p.base.as_deref().is_none_or(|x| x == content_signature(&b)) => "base holds",
            Ok(_) => "BASE CHANGED — check, then --rebase",
            Err(_) => "file not found",
        };
        println!("{name} / {}  ({state})", p.path);
        for op in &p.ops {
            match op {
                PatchOp::Set { section, key, value } => println!("    [{section}] {key} = {value}"),
                PatchOp::Remove { section, key } => println!("    [{section}] {key} — removed"),
            }
        }
    }
    Ok(())
}
