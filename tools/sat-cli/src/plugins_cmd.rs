//! `sat-cli plugins`: the plugin list of the project and edits of it — order, marks, hidden plugins. @sat:cli.plugins
//!
//! Owns: building the plugin list from the project's facts (no scan), and the command line around the plugin edits
//! (`@sat:plugins.edit`, `@sat:plugins.edit.order`).
//! Not here: the list (`@sat:plugins.list`), writing the manager's files (`@sat:cli.commit.plugins`).
//!
//! Edits go into the project only; `sat-cli commit --apply` writes `plugins.txt` / `loadorder.txt`, and an analysis
//! in between keeps them (`@sat:plugins.edit.reconcile`).

use sat_core::ids::ModId;
use sat_core::plugins::{Colour, PluginList, PluginVersions};
use sat_core::project::Project;
use sat_mo2::commit::Layout;

use sat_core::session::OVERWRITE;

use crate::{flag_value, flag_values, has, open_instance, project_io};

/// The plugin list of the project: plugin versions from its facts, priorities from its canon.
fn list_of(stll: &mut sat_stll::Stll, project: &Project) -> PluginList {
    let mut names: Vec<String> = project
        .canon
        .entries
        .iter()
        .filter(|e| e.is_mod())
        .map(|e| e.name.clone())
        .collect();
    names.push(OVERWRITE.into());
    let ids: std::collections::HashMap<String, String> = project
        .entities
        .iter()
        .map(|e| (e.name.to_lowercase(), e.id.0.clone()))
        .chain(std::iter::once((
            OVERWRITE.to_lowercase(),
            sat_stll::file::OVERWRITE_ID.to_string(),
        )))
        .collect();
    let mut versions = PluginVersions::default();
    for (i, n) in names.iter().enumerate() {
        if let Some(mut f) = ids
            .get(&n.to_lowercase())
            .and_then(|id| stll.facts(id))
            .map(|x| x.1)
        {
            // only the plugins are needed
            f.dirs.clear();
            f.archives.clear();
            versions.add(ModId::from_idx(i), &f);
        }
    }
    let index: std::collections::HashMap<String, ModId> = names
        .iter()
        .enumerate()
        .map(|(i, n)| (n.to_lowercase(), ModId::from_idx(i)))
        .collect();
    let prio = project.canon.priorities(
        names.len(),
        |n| index.get(&n.to_lowercase()).copied(),
        index.get(&OVERWRITE.to_lowercase()).copied(),
    );
    PluginList::compute(&versions, &project.plugins, &prio)
}

/// `sat-cli plugins <instance> [--move NAME]... [--before NAME | --last] [--check NAME]... [--uncheck NAME]...
/// [--hide NAME]... [--show NAME]... [--cascade] [--list]`
pub fn plugins(args: &[String]) -> Result<(), String> {
    let inst = open_instance(args)?;
    let path = Layout::of(&inst).project_file;
    let (mut stll, mut project) = project_io::require(&path)?;
    let mut list = list_of(&mut stll, &project);
    let mut changed = false;

    let moving: Vec<String> = flag_values(args, "--move")
        .into_iter()
        .map(str::to_string)
        .collect();
    if !moving.is_empty() {
        let before = flag_value(args, "--before");
        if before.is_none() && !has(args, "--last") {
            return Err("--move needs --before NAME or --last".into());
        }
        changed |= project
            .plugins
            .move_plugins(&list, &moving, before)
            .map_err(|e| format!("not moved: {e}"))?;
    }
    for (flag, on) in [("--check", true), ("--uncheck", false)] {
        for n in flag_values(args, flag) {
            if list.get(n).is_none() {
                return Err(format!("{flag} {n}: no such plugin"));
            }
            changed |= project.plugins.set_checked(n, on);
        }
    }
    for (flag, on) in [("--hide", true), ("--show", false)] {
        for n in flag_values(args, flag) {
            changed |= project.plugins.set_blacklisted(n, on);
        }
    }
    if has(args, "--cascade") {
        list = list_of(&mut stll, &project);
        let c = sat_core::plugin_edit::cascade(&list, &project.plugins);
        changed |= c != project.plugins;
        project.plugins = c;
    }
    if changed {
        project_io::save(&mut stll, &project)?;
        eprintln!(
            "saved to {} — `sat-cli commit --apply` writes plugins.txt / loadorder.txt",
            path.display()
        );
        list = list_of(&mut stll, &project);
    }

    if has(args, "--list") {
        for p in list
            .plugins
            .iter()
            .filter(|p| p.present || p.listed != sat_core::plugins::Listed::Unlisted)
        {
            let index = p
                .index
                .map_or_else(|| "  --  ".to_string(), |i| format!("{i:<6}"));
            let colour = match p.colour {
                Colour::Blue => "base",
                Colour::Green => "",
                Colour::Yellow => "OVERFLOW",
                Colour::Red => "MISSING MASTERS",
                Colour::White => "off",
                Colour::Black => "mod off",
            };
            let owner = p
                .winner
                .map(|w| p.versions[w].owner)
                .and_then(|m| project.canon.entries.iter().filter(|e| e.is_mod()).nth(m.idx()))
                .map_or_else(String::new, |e| format!("   ({})", e.name));
            let hidden = if p.blacklisted { " [hidden]" } else { "" };
            let missing = if p.missing_masters.is_empty() {
                String::new()
            } else {
                format!(" → {}", p.missing_masters.join(", "))
            };
            println!(
                "{index} {}{} {}{owner}{hidden}  {colour}{missing}",
                if p.active { "*" } else { " " },
                if p.master { "M" } else { " " },
                p.name
            );
        }
    }
    let s = list.stats;
    println!(
        "plugins={} full={} light={} overflow={}/{} missing-masters={} hidden={}",
        s.plugins,
        s.active_full,
        s.active_light,
        s.overflow_full,
        s.overflow_light,
        s.with_missing_masters,
        s.blacklisted
    );
    Ok(())
}
