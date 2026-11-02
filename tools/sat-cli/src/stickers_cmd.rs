//! `sat-cli stickers`: conditional stickers of the project. @sat:cli.stickers
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
