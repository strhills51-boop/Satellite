//! `sat-cli group` and `sat-cli factory`: logical groups and the factories that build them into one mod. @sat:cli.factory
//!
//! Owns: creating and dissolving logical groups in the project from the command line, and adding, removing and
//! listing factories with their state (never built, built, dirty).
//! Not here: groups in the order (`@sat:order.book`), factories (`@sat:factory`), building (`@sat:mo2.bake`).

use sat_core::factory::{Factory, FactoryKind, FactoryMode, fingerprint};
use sat_mo2::commit::Layout;

use crate::{flag_value, flag_values, has, open_instance, project_io};

/// `sat-cli group <instance> [--add NAME --from MOD --to MOD] [--remove NAME]`: a group is the run of mods from
/// one to the other (one block, contiguous); the analysis computes it as one mod.
pub fn group(args: &[String]) -> Result<(), String> {
    let inst = open_instance(args)?;
    let path = Layout::of(&inst).project_file;
    let (mut stll, mut project) = project_io::require(&path)?;
    let (mut book, _) = project.book();
    let mut changed = false;
    for name in flag_values(args, "--remove") {
        let d = book.decor();
        let ids = book.group_ids();
        let Some(i) = d.groups.iter().position(|g| g.name.eq_ignore_ascii_case(name)) else {
            return Err(format!("no group {name:?}"));
        };
        book.dissolve_group(ids[i]).map_err(|e| e.to_string())?;
        changed = true;
    }
    if let Some(name) = flag_value(args, "--add") {
        let (from, to) = (
            flag_value(args, "--from").ok_or("--add NAME needs --from MOD --to MOD")?,
            flag_value(args, "--to").ok_or("--add NAME needs --from MOD --to MOD")?,
        );
        let key = |n: &str| book.key_of(n).ok_or(format!("no mod {n:?} in the project"));
        let (a, b) = (book.pos(key(from)?), book.pos(key(to)?));
        let keys: Vec<_> = (a.min(b)..=a.max(b)).map(|p| book.key_at(p)).collect();
        book.create_group(name, &keys).map_err(|e| e.to_string())?;
        changed = true;
    }
    if changed {
        project.commit_book(&book);
        project_io::save(&mut stll, &project)?;
        eprintln!("saved to {}", path.display());
    }
    let groups = book.decor().groups;
    if groups.is_empty() {
        println!("no logical groups (add one: --add NAME --from MOD --to MOD)");
    }
    for g in &groups {
        println!(
            "{}  [{}]  {} mod(s): {} … {}",
            g.name,
            g.id,
            g.members.len(),
            g.members.first().map_or("", String::as_str),
            g.members.last().map_or("", String::as_str)
        );
    }
    Ok(())
}

// @sat:cli.factory.run
/// `sat-cli factory <instance> [--add "GROUP=OUTPUT" [--link] [--keep-members] [--seal]] [--remove OUTPUT]`: the
/// next `sat-cli commit --apply` builds what is new or changed. `--seal` — bake once and take the members out of the
/// build (the commit asks for `--confirm "OUTPUT"`).
pub fn factory(args: &[String]) -> Result<(), String> {
    let inst = open_instance(args)?;
    let path = Layout::of(&inst).project_file;
    let (mut stll, mut project) = project_io::require(&path)?;
    let groups = project.book().0.decor().groups;
    let mut changed = false;
    for out in flag_values(args, "--remove") {
        let before = project.factories.len();
        project.factories.retain(|f| !f.output.eq_ignore_ascii_case(out));
        changed |= before != project.factories.len();
    }
    if let Some(a) = flag_value(args, "--add") {
        let (g, output) = a
            .split_once('=')
            .ok_or_else(|| format!("--add {a:?}: GROUP=OUTPUT, e.g. \"Costumes=Costumes (baked)\""))?;
        let group = groups
            .iter()
            .find(|x| x.name.eq_ignore_ascii_case(g.trim()) || x.id == g.trim())
            .ok_or_else(|| format!("no group {:?} (sat-cli group lists them)", g.trim()))?;
        if project.entities.id_of(output.trim()).is_some()
            && !project
                .factories
                .iter()
                .any(|f| f.output.eq_ignore_ascii_case(output.trim()))
        {
            return Err(format!(
                "{:?} is a mod of the project already: pick another output name",
                output.trim()
            ));
        }
        project
            .factories
            .retain(|f| !f.output.eq_ignore_ascii_case(output.trim()));
        project.factories.push(Factory {
            group: group.id.clone(),
            output: output.trim().to_string(),
            kind: FactoryKind::Bake,
            link: has(args, "--link"),
            keep_members: has(args, "--keep-members"),
            mode: if has(args, "--seal") {
                FactoryMode::Seal
            } else {
                FactoryMode::Live
            },
            built: None,
        });
        changed = true;
    }
    if changed {
        project_io::save(&mut stll, &project)?;
        eprintln!(
            "saved to {} — `sat-cli commit` shows what will be built",
            path.display()
        );
    }
    if project.factories.is_empty() {
        println!("no factories (add one: --add \"GROUP=OUTPUT\" [--link])");
    }
    for f in &project.factories {
        let state = match groups.iter().find(|g| g.id == f.group) {
            None => "its group is gone".to_string(),
            Some(g) => {
                let sigs: Vec<(String, Option<u64>)> = g
                    .members
                    .iter()
                    .map(|m| {
                        (
                            m.clone(),
                            project
                                .entities
                                .id_of(m)
                                .and_then(|id| project.signatures.get(&id.0))
                                .copied(),
                        )
                    })
                    .collect();
                match &f.built {
                    None => format!("never built — {} member(s) of {}", g.members.len(), g.name),
                    Some(b) if *b == fingerprint(f, &sigs) => format!("built — {} is up to date", g.name),
                    Some(_) => format!("dirty — {} changed since the last build", g.name),
                }
            }
        };
        println!(
            "{}  ←  bake{}{}{}  ({state})",
            f.output,
            if f.mode == FactoryMode::Seal {
                " and seal"
            } else {
                ""
            },
            if f.link { ", links" } else { "" },
            if f.keep_members { ", members kept" } else { "" }
        );
    }
    Ok(())
}
