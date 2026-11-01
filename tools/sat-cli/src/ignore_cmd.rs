//! `sat-cli ignore`: the conflict ignore list of the project. @sat:cli.ignore
//!
//! Owns: listing, adding and removing conflict ignore rules in the project file.
//! Not here: the rules and their forms (`@sat:ignore`), applying them (`analyze`, `@sat:overwrite.conflicts`).

use sat_core::ignore::IgnoreRules;
use sat_mo2::commit::Layout;

use crate::{flag_values, open_instance, project_io};

/// `sat-cli ignore <instance> [--add RULE]... [--remove RULE]...`; the next `analyze` applies the list.
pub fn ignore(args: &[String]) -> Result<(), String> {
    let inst = open_instance(args)?;
    let path = Layout::of(&inst).project_file;
    let (mut stll, mut project) = project_io::require(&path)?;
    let (add, remove) = (flag_values(args, "--add"), flag_values(args, "--remove"));
    if !add.is_empty() || !remove.is_empty() {
        let gone = IgnoreRules::new(remove);
        let rules = project
            .conflict_ignore
            .rules()
            .iter()
            .map(String::as_str)
            .filter(|r| !gone.rules().iter().any(|g| g == r))
            .chain(add)
            .collect::<Vec<_>>();
        project.conflict_ignore = IgnoreRules::new(rules);
        project_io::save(&mut stll, &project)?;
        eprintln!("saved to {} — run `sat-cli analyze` to apply", path.display());
    }
    if project.conflict_ignore.is_empty() {
        println!(
            "no ignore rules (forms: `meta.ini` — a file anywhere, `docs/` — a folder anywhere, `a/b.dds` — one path)"
        );
    }
    for r in project.conflict_ignore.rules() {
        println!("  {r}");
    }
    Ok(())
}
