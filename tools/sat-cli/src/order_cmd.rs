//! `sat-cli order`: the mod order from the command line — service separators, filing mods. @sat:cli.order
//!
//! Owns: adding a separator to the project's canon (a service one such as `obsolete` or `review`: the commit makes
//! its folder), moving mods to the end of a separator's block or before an entry, filing mods into a logical group
//! or taking them out, and listing the separators with how many mods each holds. Everything goes into the project
//! only; `sat-cli commit` writes `modlist.txt` and makes the folders.
//! Not here: the order itself (`@sat:order.book`), the plan (`@sat:sync.plan`).
//!
//! The owner sorts his build by intentions — mods filed under separators and groups — not by moving folders on disk.

use sat_core::canon::{Entry, EntryKind, SEPARATOR_SUFFIX};
use sat_core::order::{Key, OrderBook};
use sat_mo2::commit::Layout;

use crate::{flag_value, flag_values, has, open_instance};

fn key(book: &OrderBook, name: &str) -> Result<Key, String> {
    book.key_of(name)
        .ok_or_else(|| format!("no entry {name:?} in the project"))
}

/// A separator by its name with or without the suffix.
fn separator(book: &OrderBook, name: &str) -> Result<Key, String> {
    let full = if name.to_lowercase().ends_with(SEPARATOR_SUFFIX) {
        name.to_string()
    } else {
        format!("{name}{SEPARATOR_SUFFIX}")
    };
    let k = key(book, &full)?;
    if book.kind(k) != EntryKind::Separator {
        return Err(format!("{full:?} is not a separator"));
    }
    Ok(k)
}

/// The entry right after a separator's block (the next separator), `None` — the block runs to the end.
fn after_block(book: &OrderBook, sep: Key) -> Option<Key> {
    let entries: Vec<(String, EntryKind)> = book.entries().map(|(n, k, _)| (n.to_string(), k)).collect();
    let at = entries.iter().position(|(n, _)| n == book.name(sep))?;
    entries[at + 1..]
        .iter()
        .find(|(_, k)| *k == EntryKind::Separator)
        .and_then(|(n, _)| book.key_of(n))
}

// @sat:cli.order.run
/// `sat-cli order <instance> [--add-separator NAME [--before ENTRY]] [--move MOD]... (--under SEPARATOR | --before
/// ENTRY) [--into-group MOD --join MOD]... [--out-of-group MOD]... [--separators]`
pub fn order(args: &[String]) -> Result<(), String> {
    let inst = open_instance(args)?;
    let path = Layout::of(&inst).project_file;
    let (mut stll, mut project) = sat_home::project::require(&path)?;
    let (mut book, _) = project.book();
    let mut changed = false;
    let before = |book: &OrderBook| flag_value(args, "--before").map(|b| key(book, b)).transpose();

    if let Some(name) = flag_value(args, "--add-separator") {
        let base = name.trim();
        let base = base.strip_suffix(SEPARATOR_SUFFIX).unwrap_or(base).trim();
        if base.is_empty() || base.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|']) {
            return Err(format!("--add-separator {name:?}: a folder name is needed"));
        }
        let entry = Entry {
            name: format!("{base}{SEPARATOR_SUFFIX}"),
            kind: EntryKind::Separator,
            enabled: false,
        };
        let at = before(&book)?;
        book.insert_entries(std::slice::from_ref(&entry), at)
            .map_err(|e| e.to_string())?;
        println!("separator {} added", entry.name);
        changed = true;
    }

    let moved = flag_values(args, "--move");
    if !moved.is_empty() {
        let keys = moved
            .iter()
            .map(|m| key(&book, m))
            .collect::<Result<Vec<_>, _>>()?;
        let target = match (flag_value(args, "--under"), before(&book)?) {
            (Some(s), _) => after_block(&book, separator(&book, s)?),
            (None, Some(b)) => Some(b),
            (None, None) => return Err("--move needs --under SEPARATOR or --before ENTRY".into()),
        };
        book.move_keys(&keys, target).map_err(|e| e.to_string())?;
        println!(
            "moved {} mod(s){}",
            keys.len(),
            flag_value(args, "--under").map_or(String::new(), |s| format!(" to the end of {s}"))
        );
        changed = true;
    }

    // filing into a group: --into-group MOD --join MEMBER (a member names the group)
    for w in args.windows(4) {
        if w[0] == "--into-group" && w[2] == "--join" {
            let (m, member) = (key(&book, &w[1])?, key(&book, &w[3])?);
            let g = book
                .group_id_of(member)
                .ok_or_else(|| format!("{:?} is not in a logical group", w[3]))?;
            book.regroup(g, &[m], &[]).map_err(|e| format!("{}: {e}", w[1]))?;
            println!("{} filed into the group of {}", w[1], w[3]);
            changed = true;
        }
    }
    for m in flag_values(args, "--out-of-group") {
        let k = key(&book, m)?;
        let g = book
            .group_id_of(k)
            .ok_or_else(|| format!("{m:?} is not in a logical group"))?;
        book.regroup(g, &[], &[k]).map_err(|e| format!("{m}: {e}"))?;
        println!("{m} taken out of its group");
        changed = true;
    }

    if changed {
        project.commit_book(&book);
        sat_home::project::save(&mut stll, &project)?;
        eprintln!(
            "saved to {} (sat-cli commit writes modlist.txt and makes new folders)",
            path.display()
        );
    }
    if has(args, "--separators") || !changed {
        let mut cur: Option<(String, usize, usize)> = None;
        let mut out = Vec::new();
        for (n, k, en) in book.entries() {
            if k == EntryKind::Separator {
                out.extend(cur.take());
                cur = Some((n.to_string(), 0, 0));
            } else if let Some(c) = cur.as_mut() {
                c.1 += 1;
                c.2 += usize::from(en);
            }
        }
        out.extend(cur);
        for (n, mods, en) in out {
            println!("{mods:>6} mod(s), {en:>5} enabled   {n}");
        }
    }
    Ok(())
}
