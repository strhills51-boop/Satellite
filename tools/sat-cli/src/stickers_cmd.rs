//! `sat-cli stickers` and `sat-cli counters`: conditional stickers and user-made counters of the project. @sat:cli.stickers
//!
//! Owns: listing, adding and removing sticker rules (`sticker = expression`) in the project file.
//! Not here: the rules (`@sat:sticker-rules`), the language (`@sat:expr`).

use sat_core::sticker_rules::{StickerRule, StickerRules};
use sat_mo2::commit::Layout;

use crate::{flag_values, open_instance, project_io};

/// `sat-cli stickers <instance> [--add "STICKER=EXPR"]... [--remove STICKER]...`: a rule that does not compile is
/// refused.
pub fn stickers(args: &[String]) -> Result<(), String> {
    let inst = open_instance(args)?;
    let path = Layout::of(&inst).project_file;
    let (mut stll, mut project) = project_io::require(&path)?;
    let (add, remove) = (flag_values(args, "--add"), flag_values(args, "--remove"));
    if !add.is_empty() || !remove.is_empty() {
        project
            .sticker_rules
            .retain(|r| !remove.iter().any(|x| x.eq_ignore_ascii_case(&r.sticker)));
        for a in add {
            let (sticker, when) = a
                .split_once('=')
                .ok_or_else(|| format!("--add {a:?}: STICKER=EXPRESSION, e.g. \"heavy=size > 1 GB\""))?;
            let rule = StickerRule {
                sticker: sticker.trim().to_string(),
                when: when.trim().to_string(),
            };
            if let Some(e) = StickerRules::compile(std::slice::from_ref(&rule)).1.first() {
                return Err(e.clone());
            }
            project.sticker_rules.retain(|r| r.sticker != rule.sticker);
            project.sticker_rules.push(rule);
        }
        project_io::save(&mut stll, &project)?;
        eprintln!("saved to {}", path.display());
    }
    if project.sticker_rules.is_empty() {
        println!("no sticker rules (add one: --add \"heavy=size > 1 GB\")");
    }
    for r in &project.sticker_rules {
        println!("{}  =  {}", r.sticker, r.when);
    }
    Ok(())
}

// @sat:cli.counters
/// `sat-cli counters <instance> [--add "ID=TEMPLATE"]... [--case "ID|CONDITION|TONE"]... [--show-if
/// "ID|CONDITION"]... [--remove ID]...`: the counter constructor (`@sat:counters`); a definition that does not
/// compile is refused.
pub fn counters(args: &[String]) -> Result<(), String> {
    use sat_core::counters::{Case, CounterDef, Counters};
    let inst = open_instance(args)?;
    let path = Layout::of(&inst).project_file;
    let (mut stll, mut project) = project_io::require(&path)?;
    let changed = ["--add", "--case", "--show-if", "--remove"]
        .iter()
        .any(|f| !flag_values(args, f).is_empty());
    for r in flag_values(args, "--remove") {
        project.counters.retain(|c| c.id != r);
    }
    for a in flag_values(args, "--add") {
        let (id, template) = a
            .split_once('=')
            .ok_or_else(|| format!("--add {a:?}: ID=TEMPLATE, e.g. \"wl={{wins}}/{{loses}}\""))?;
        project.counters.retain(|c| c.id != id.trim());
        project.counters.push(CounterDef {
            id: id.trim().into(),
            template: template.into(),
            cases: Vec::new(),
            show_if: String::new(),
        });
    }
    let find = |project: &sat_core::project::Project, id: &str| {
        project
            .counters
            .iter()
            .position(|c| c.id == id.trim())
            .ok_or(format!("no counter {:?}", id.trim()))
    };
    for c in flag_values(args, "--case") {
        let p: Vec<&str> = c.splitn(3, '|').collect();
        let [id, when, tone] = p[..] else {
            return Err(format!("--case {c:?}: ID|CONDITION|TONE"));
        };
        let i = find(&project, id)?;
        project.counters[i].cases.push(Case {
            when: when.into(),
            tone: tone.trim().into(),
        });
    }
    for c in flag_values(args, "--show-if") {
        let Some((id, when)) = c.split_once('|') else {
            return Err(format!("--show-if {c:?}: ID|CONDITION"));
        };
        let i = find(&project, id)?;
        project.counters[i].show_if = when.into();
    }
    if changed {
        if let Some(e) = Counters::compile(&project.counters).1.first() {
            return Err(e.clone());
        }
        project_io::save(&mut stll, &project)?;
        eprintln!("saved to {}", path.display());
    }
    if project.counters.is_empty() {
        println!("no counters (add one: --add \"wl={{wins}}/{{loses}}\")");
    }
    for c in &project.counters {
        println!("{}  =  {}", c.id, c.template);
        for k in &c.cases {
            println!("    when {}  →  {}", k.when, k.tone);
        }
        if !c.show_if.is_empty() {
            println!("    shown if {}", c.show_if);
        }
    }
    Ok(())
}
