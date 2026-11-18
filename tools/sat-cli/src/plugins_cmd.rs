//! `sat-cli plugins`: the plugin list of the project and edits of it — order, marks, hidden plugins. @sat:cli.plugins
//!
//! Owns: building the plugin list from the project's facts (no scan), and the command line around the plugin edits
//! (`@sat:plugins.edit`, `@sat:plugins.edit.order`).
//! Not here: the list (`@sat:plugins.list`), writing the manager's files (`@sat:home.plan.plugins`).
//!
//! Edits go into the project only; `sat-cli commit --apply` writes `plugins.txt` / `loadorder.txt`, and an analysis
//! in between keeps them (`@sat:plugins.edit.reconcile`).

use sat_core::ids::ModId;
use sat_core::plugins::{Caste, Colour, PluginList, PluginState, PluginVersions};
use sat_core::project::Project;
use sat_mo2::commit::Layout;

use sat_core::session::OVERWRITE;

use crate::{flag_value, flag_values, has, open_instance};

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
/// [--hide NAME]... [--show NAME]... [--drop-broken] [--cascade] [--list] [--tsv FILE]`
pub fn plugins(args: &[String]) -> Result<(), String> {
    let inst = open_instance(args)?;
    let path = Layout::of(&inst).project_file;
    let (mut stll, mut project) = sat_home::project::require(&path)?;
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
    // @sat:cli.plugins.drop-broken
    // plugins that cannot load (a master missing or not active) are unchecked, and those that load only on them;
    // nothing is turned on. One line per plugin: plugin, its mod, the masters it lacks (tab-separated).
    if has(args, "--drop-broken") {
        let (c, dropped) = sat_core::plugin_edit::drop_broken(&list, &project.plugins);
        for d in &dropped {
            let owner = list
                .get(&d.plugin)
                .and_then(|p| p.winner.map(|w| p.versions[w].owner))
                .and_then(|m| project.canon.entries.iter().filter(|e| e.is_mod()).nth(m.idx()))
                .map_or(String::new(), |e| e.name.clone());
            println!("dropped\t{}\t{}\t{}", d.plugin, owner, d.lacks.join(", "));
        }
        eprintln!("{} plugin(s) cannot load and are unchecked", dropped.len());
        changed |= c != project.plugins;
        project.plugins = c;
    }
    if has(args, "--cascade") {
        list = list_of(&mut stll, &project);
        let c = sat_core::plugin_edit::cascade(&list, &project.plugins);
        changed |= c != project.plugins;
        project.plugins = c;
    }
    if changed {
        sat_home::project::save(&mut stll, &project)?;
        eprintln!(
            "saved to {} — `sat-cli commit --apply` writes plugins.txt / loadorder.txt",
            path.display()
        );
        list = list_of(&mut stll, &project);
    }

    let mod_name = |m: ModId| {
        project
            .canon
            .entries
            .iter()
            .filter(|e| e.is_mod())
            .nth(m.idx())
            .map_or_else(|| OVERWRITE.to_string(), |e| e.name.clone())
    };
    if has(args, "--list") {
        for p in list
            .plugins
            .iter()
            .filter(|p| p.present || p.listed != sat_core::plugins::Listed::Unlisted)
        {
            let index = p
                .index
                .map_or_else(|| "  --  ".to_string(), |i| format!("{i:<6}"));
            let owner = p
                .winner
                .map_or_else(String::new, |w| format!("   ({})", mod_name(p.versions[w].owner)));
            // the priority first, then every other state (they stack)
            let states: Vec<&str> = p.states.iter().map(|s| state_label(*s)).collect();
            let colour = match p.colour {
                Colour::Blue => "base",
                Colour::Green => "",
                Colour::Yellow => "OVERFLOW",
                Colour::Red => "MISSING MASTERS",
                Colour::Grey => "off",
                Colour::Black => "mod off",
                Colour::Ghost => "GHOST",
            };
            let missing = if p.missing_masters.is_empty() {
                String::new()
            } else {
                let why: Vec<String> = p
                    .missing_masters
                    .iter()
                    .zip(&p.master_gaps)
                    .map(|(m, g)| format!("{m} [{}]", state_label(PluginState::from(*g))))
                    .collect();
                format!(" → {}", why.join(", "))
            };
            println!(
                "{index} {}{} {}{owner}  {colour}{}{missing}",
                if p.active { "*" } else { " " },
                if p.master { "M" } else { " " },
                p.name,
                if states.is_empty() {
                    String::new()
                } else {
                    format!(" [{}]", states.join(" "))
                }
            );
        }
        for g in &list.ghosts {
            println!("  --     ? {}  GHOST ← {}", g.name, g.needed_by.join(", "));
        }
    }
    // @sat:cli.plugins.tsv
    // the whole list as a table, one plugin per line, ghosts last: for analysis outside (a plugin, not a mod, is the unit)
    if let Some(file) = flag_value(args, "--tsv") {
        let mut out = String::from(
            "plugin\tcaste\tcolour\tstates\tindex\tload_order\tchecked\tactive\twinner\tversions\tmissing_masters\tmasters\n",
        );
        for p in &list.plugins {
            let caste = match p.caste() {
                Caste::Esm => "esm",
                Caste::Esp => "esp",
                Caste::Esl => "esl",
            };
            let colour = serde_json::to_value(p.colour)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default();
            let states: Vec<&str> = p.states.iter().map(|s| state_label(*s)).collect();
            let versions: Vec<String> = p.versions.iter().map(|v| mod_name(v.owner)).collect();
            let missing: Vec<String> = p
                .missing_masters
                .iter()
                .zip(&p.master_gaps)
                .map(|(m, g)| format!("{m}={}", state_label(PluginState::from(*g))))
                .collect();
            out.push_str(&format!(
                "{}\t{caste}\t{colour}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                p.name,
                states.join(" "),
                p.index.map(|i| i.to_string()).unwrap_or_default(),
                p.load_order.map(|i| i.to_string()).unwrap_or_default(),
                p.listed != sat_core::plugins::Listed::Disabled,
                p.active,
                p.winner
                    .map(|w| mod_name(p.versions[w].owner))
                    .unwrap_or_default(),
                versions.join(" | "),
                missing.join(" | "),
                p.masters.join(" | "),
            ));
        }
        for g in &list.ghosts {
            out.push_str(&format!(
                "{}\t\tghost\tghost\t\t\tfalse\tfalse\t\t\t\tneeded by: {}\n",
                g.name,
                g.needed_by.join(" | ")
            ));
        }
        std::fs::write(file, out).map_err(|e| format!("{file}: {e}"))?;
        eprintln!("plugins → {file}");
    }
    let s = list.stats;
    println!(
        "plugins={} full={} light={} overflow={}/{} missing-masters={} (waiting only for mods that are off: {}) unchecked={} hidden={} ghosts={}",
        s.plugins,
        s.active_full,
        s.active_light,
        s.overflow_full,
        s.overflow_light,
        s.with_missing_masters,
        s.waiting_for_mods_off,
        s.unchecked,
        s.blacklisted,
        s.ghosts
    );
    Ok(())
}

/// A state as the command line writes it.
fn state_label(s: PluginState) -> &'static str {
    match s {
        PluginState::Missing => "missing",
        PluginState::OwnerOff => "mod-off",
        PluginState::Hidden => "hidden",
        PluginState::MasterAbsent => "master-absent",
        PluginState::MasterOwnerOff => "master-mod-off",
        PluginState::MasterUnchecked => "master-unchecked",
        PluginState::Overflow => "overflow",
        PluginState::Unchecked => "unchecked",
        PluginState::Unlisted => "unlisted",
        PluginState::Versioned => "versioned",
    }
}
