//! `sat-cli processors`: the build's generators from the command line (ADR 0021). @sat:cli.processors
//!
//! Owns: listing the project's processors with their state against the build (clean / dirty and why / no tool /
//! off / cycle) and inputs, MO2's registered tools, taking in the ones the presets recognise (`--detect`), turning
//! them on and off. A processor runs only in `sat-cli commit --apply`.
//! Not here: what is dirty (`@sat:home.plan.processors.assess`), the run (`@sat:mo2.processors`).

use sat_core::processor::{State, detect};
use sat_mo2::commit::Layout;

use crate::{flag_values, has, open_instance};

/// `sat-cli processors <instance> [--detect] [--enable ID]... [--disable ID]...`
pub fn processors(args: &[String]) -> Result<(), String> {
    let inst = open_instance(args)?;
    let path = Layout::of(&inst).project_file;
    let (mut stll, mut project) = sat_home::project::require(&path)?;
    let mut changed = false;
    if has(args, "--detect") {
        let (_, tools) = sat_home::plan::processor_status(&inst, &project);
        let found = detect(&tools, &project.processors);
        for p in &found {
            println!(
                "found {} — MO2 tool «{}», output {}",
                p.title, p.executable, p.output
            );
        }
        changed |= !found.is_empty();
        project.processors.extend(found);
    }
    for (flag, on) in [("--enable", true), ("--disable", false)] {
        for id in flag_values(args, flag) {
            let p = project
                .processors
                .iter_mut()
                .find(|p| p.id.eq_ignore_ascii_case(id))
                .ok_or_else(|| format!("{flag} {id}: no such processor (sat-cli processors … lists them)"))?;
            p.enabled = on;
            changed = true;
        }
    }
    if changed {
        sat_home::project::save(&mut stll, &project)?;
        eprintln!("processors saved to {}", path.display());
    }
    let (states, tools) = sat_home::plan::processor_status(&inst, &project);
    println!(
        "MO2 tools: {}",
        if tools.is_empty() {
            "none registered".to_string()
        } else {
            tools
                .iter()
                .map(|(t, _)| t.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        }
    );
    if project.processors.is_empty() {
        println!("no processors (--detect takes in the generators recognised among MO2's tools)");
    }
    for (p, s) in project.processors.iter().zip(&states) {
        let state = match &s.state {
            State::Off => "off".to_string(),
            State::NoTool => format!("no tool «{}» in MO2", p.executable),
            State::Cycle { with } => format!("cycle: {}", with.join(" → ")),
            State::Clean => "clean".to_string(),
            State::Dirty { why } => format!(
                "dirty: {} — the next commit runs it",
                serde_json::to_string(why).unwrap_or_default().replace('"', "")
            ),
        };
        println!(
            "{:<12} {} → {}  [{state}]  inputs {}{}",
            p.id,
            p.executable,
            p.output,
            s.inputs.len(),
            if p.after.is_empty() {
                String::new()
            } else {
                format!(", after {}", p.after.join(", "))
            }
        );
    }
    Ok(())
}
