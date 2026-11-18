//! `sat-cli forks`: forks of taste in the project — added, chosen, listed. @sat:cli.forks
//!
//! Owns: adding forks from a file (JSON lines), choosing an option (its mods on, the mods only the other options
//! have off — in the project; `sat-cli commit` writes the list), who chose it (Claude by default, `--owner` for the
//! owner's final word), listing them and writing them as a readable page.
//! Not here: the fork itself and what a choice asks of the list (`@sat:forks`).

use sat_core::forks::{ChosenBy, Fork};
use sat_mo2::commit::Layout;

use crate::{flag_value, flag_values, has, open_instance};

// @sat:cli.forks.run
/// `sat-cli forks <instance> [--add FILE] [--choose ID N [--owner] [--why TEXT]]... [--remove ID]... [--page FILE]`
pub fn forks(args: &[String]) -> Result<(), String> {
    let inst = open_instance(args)?;
    let path = Layout::of(&inst).project_file;
    let (mut stll, mut project) = sat_home::project::require(&path)?;
    let (mut book, _) = project.book();
    let mut changed = false;
    let mut problems: Vec<String> = Vec::new();

    // the list follows a choice: the chosen option's mods on, the others' off
    let apply =
        |book: &mut sat_core::order::OrderBook, f: &Fork, i: usize, problems: &mut Vec<String>| {
            let Some(e) = f.effect(i) else {
                problems.push(format!("{}: no option {i}", f.id));
                return;
            };
            for (names, on) in [(&e.enable, true), (&e.disable, false)] {
                let keys: Vec<_> = names
                    .iter()
                    .filter_map(|n| {
                        let k = book.key_of(n);
                        if k.is_none() {
                            problems.push(format!("{}: no mod {n:?} in the project", f.id));
                        }
                        k
                    })
                    .collect();
                if !keys.is_empty()
                    && let Err(err) = book.set_enabled(&keys, Some(on))
                {
                    problems.push(format!("{}: {err}", f.id));
                }
            }
        };

    for id in flag_values(args, "--remove") {
        let before = project.forks.len();
        project.forks.retain(|f| f.id != *id);
        if project.forks.len() == before {
            return Err(format!("no fork {id:?}"));
        }
        changed = true;
    }

    if let Some(file) = flag_value(args, "--add") {
        let text = std::fs::read_to_string(file).map_err(|e| format!("{file}: {e}"))?;
        for (n, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let mut f: Fork = serde_json::from_str(line).map_err(|e| format!("{file}:{}: {e}", n + 1))?;
            // a fork for the same aspect is replaced (a sweep can be run again)
            project
                .forks
                .retain(|o| !o.aspect.eq_ignore_ascii_case(&f.aspect));
            if f.id.is_empty() || project.forks.iter().any(|o| o.id == f.id) {
                f.id = sat_core::forks::fresh_id(&project.forks);
            }
            if let Some(i) = f.chosen {
                apply(&mut book, &f, i, &mut problems);
            }
            project.forks.push(f);
            changed = true;
        }
    }

    for w in args.windows(3) {
        if w[0] != "--choose" {
            continue;
        }
        let i: usize = w[2]
            .parse()
            .map_err(|_| format!("--choose {} {}: an option number", w[1], w[2]))?;
        let Some(pos) = project.forks.iter().position(|f| f.id == w[1]) else {
            return Err(format!("no fork {:?}", w[1]));
        };
        let f = project.forks[pos].clone();
        apply(&mut book, &f, i, &mut problems);
        let f = &mut project.forks[pos];
        f.chosen = Some(i);
        f.by = if has(args, "--owner") {
            ChosenBy::Owner
        } else {
            ChosenBy::Claude
        };
        if let Some(why) = flag_value(args, "--why") {
            f.why = why.to_string();
        }
        changed = true;
    }

    for p in &problems {
        eprintln!("{p}");
    }
    if changed {
        project.commit_book(&book);
        sat_home::project::save(&mut stll, &project)?;
        eprintln!("saved to {} (sat-cli commit writes modlist.txt)", path.display());
    }

    if let Some(page) = flag_value(args, "--page") {
        std::fs::write(page, markdown(&project.forks)).map_err(|e| format!("{page}: {e}"))?;
        eprintln!("forks → {page}");
    }
    for f in &project.forks {
        let by = match f.by {
            ChosenBy::Nobody => "не выбрано",
            ChosenBy::Claude => "выбор Claude",
            ChosenBy::Owner => "решение владельца",
        };
        println!("{}  {}  ({by}, вариантов {})", f.id, f.aspect, f.options.len());
        for (i, o) in f.options.iter().enumerate() {
            let mark = if f.chosen == Some(i) { "*" } else { " " };
            println!("   {mark} {i}. {}  [{}]", o.name, o.mods.join(" | "));
        }
    }
    Ok(())
}

/// The forks as a page for the owner: aspect, what it is about, options with their facts, the choice and why.
fn markdown(forks: &[Fork]) -> String {
    let mut s = String::from(
        "# Развилки вкуса\n\nВыбор Claude применён; финальное слово — владельца (`sat-cli forks … --choose ID N --owner`).\n",
    );
    for f in forks {
        let by = match f.by {
            ChosenBy::Nobody => "не выбрано",
            ChosenBy::Claude => "выбор Claude",
            ChosenBy::Owner => "решение владельца",
        };
        s.push_str(&format!("\n## {} — {} ({by})\n\n", f.id, f.aspect));
        if !f.about.is_empty() {
            s.push_str(&format!("{}\n\n", f.about));
        }
        for (i, o) in f.options.iter().enumerate() {
            let mark = if f.chosen == Some(i) {
                " ← в силе"
            } else {
                ""
            };
            s.push_str(&format!("{i}. **{}**{mark}: {}", o.name, o.mods.join(", ")));
            if !o.facts.is_empty() {
                s.push_str(&format!(" — {}", o.facts));
            }
            s.push('\n');
        }
        if !f.why.is_empty() {
            s.push_str(&format!("\nПочему: {}\n", f.why));
        }
    }
    s
}
