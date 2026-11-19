//! `analyze --kin FILE`: how the mods of the build are related — kin and who covers whom (ADR 0023). @sat:cli.kin
//!
//! Owns: gathering the pair facts from the provider table (paths both mods provide, their bytes on each side, the
//! same bytes by the content hashes), the candidates found without shared paths (one Nexus id, one name base), the
//! sides of each mod (on, source, game bytes, plugins and their masters), and the report: links by kind, families
//! with their base and usurpers.
//! Not here: the rules (`@sat:kin`), hashing (`@sat:cli.identical`).

use std::io::Write;

use rustc_hash::FxHashMap;
use sat_core::identical::ContentHashes;
use sat_core::ids::{ModId, PathId};
use sat_core::kin::{self, Kind, Link, Side, Surface};
use sat_core::overwrite::Priorities;
use sat_core::plugins::PluginList;
use sat_core::table::ProviderTable;

/// A path held by more mods than this is a shared resource: its pairs are not walked (its bytes still count).
const COMMON: usize = 40;

// @sat:cli.kin.surface
/// Whether a path is part of a mod's game files (not its `meta.ini`, installer, readme or pictures in the root).
pub(crate) fn game_file(path: &str) -> bool {
    let low = path.to_ascii_lowercase();
    let low = low.trim_start_matches('/');
    if low == "meta.ini" || low.starts_with("fomod/") || low.contains("readme") {
        return false;
    }
    let root = !low.contains('/');
    let ext = low.rsplit_once('.').map_or("", |(_, e)| e);
    !(matches!(
        ext,
        "txt" | "md" | "pdf" | "html" | "htm" | "url" | "rtf" | "docx" | "doc"
    ) || (root && matches!(ext, "jpg" | "jpeg" | "png" | "webp" | "gif" | "bmp")))
}

// @sat:cli.kin.run
/// The report of `analyze --kin FILE`: every link, then the families.
pub(crate) fn report(
    table: &ProviderTable,
    hashes: Option<&ContentHashes>,
    prio: &Priorities,
    sources: &std::collections::HashMap<String, sat_core::source::ModSource>,
    plugins: &PluginList,
    out: &str,
) -> Result<(), String> {
    let t = std::time::Instant::now();
    let n = table.mod_count();
    let mut sides: Vec<Side> = (0..n)
        .map(|i| {
            let m = ModId::from_idx(i);
            let name = table.mod_name(m).to_string();
            let src = sources.get(&name.to_lowercase());
            Side {
                enabled: prio.is_active(m),
                mod_id: src.and_then(|s| s.mod_id),
                version: src.and_then(|s| s.version.clone()),
                file: src.and_then(|s| s.file.clone()),
                name,
                ..Side::default()
            }
        })
        .collect();
    for p in &plugins.plugins {
        for v in &p.versions {
            let s = &mut sides[v.owner.idx()];
            s.plugins.push(p.name.to_lowercase());
            s.masters.extend(v.masters.iter().map(|m| m.to_lowercase()));
        }
    }

    // the surface of every pair that shares paths; a mod's game bytes
    let mut pairs: FxHashMap<(u32, u32), Surface> = FxHashMap::default();
    let mut owners: Vec<(u32, usize)> = Vec::new();
    for p in 0..table.path_count() {
        let pid = PathId::from_idx(p);
        if !game_file(&table.path_string(pid)) {
            continue;
        }
        // one provider per mod: its loose file before its archive entry
        owners.clear();
        for i in table.path_providers(pid) {
            let m = table.owner(i).0;
            match owners.iter_mut().find(|(o, _)| *o == m) {
                Some(slot) if table.is_archive(slot.1) && !table.is_archive(i) => slot.1 = i,
                Some(_) => {}
                None => owners.push((m, i)),
            }
        }
        for &(m, i) in &owners {
            sides[m as usize].bytes += table.size(i);
        }
        if owners.len() < 2 || owners.len() > COMMON {
            continue;
        }
        owners.sort_unstable();
        for x in 0..owners.len() {
            for y in x + 1..owners.len() {
                let ((a, ia), (b, ib)) = (owners[x], owners[y]);
                let s = pairs.entry((a, b)).or_default();
                let (sa, sb) = (table.size(ia), table.size(ib));
                s.files += 1;
                s.bytes_a += sa;
                s.bytes_b += sb;
                match hashes.and_then(|h| sat_core::identical::same(table, h, ia, ib)) {
                    Some(true) => s.same += sa,
                    Some(false) => s.differ += sa,
                    None => {}
                }
            }
        }
    }
    // candidates that share no path: one Nexus id, one name base (addons with only new files)
    let mut by: FxHashMap<String, Vec<u32>> = FxHashMap::default();
    for (i, s) in sides.iter().enumerate() {
        if s.bytes == 0 {
            continue;
        }
        if let Some(id) = s.mod_id {
            by.entry(format!("id:{id}")).or_default().push(i as u32);
        }
        let base = kin::base_name(&s.name);
        if base.len() >= 5 {
            by.entry(format!("name:{base}")).or_default().push(i as u32);
        }
    }
    for g in by.values().filter(|g| g.len() >= 2 && g.len() <= COMMON) {
        for x in 0..g.len() {
            for y in x + 1..g.len() {
                let (a, b) = (g[x].min(g[y]), g[x].max(g[y]));
                pairs.entry((a, b)).or_default();
            }
        }
    }
    let mut links: Vec<Link> = pairs
        .iter()
        .filter_map(|(&(a, b), s)| kin::classify(a, &sides[a as usize], b, &sides[b as usize], s))
        .collect();
    links.sort_by(|x, y| {
        (x.kind as u8, &sides[x.a as usize].name).cmp(&(y.kind as u8, &sides[y.a as usize].name))
    });
    let fams = kin::families(&links, &sides);

    // the report
    let mut w = std::io::BufWriter::new(std::fs::File::create(out).map_err(|e| format!("{out}: {e}"))?);
    let pct = |x: f32| format!("{:.0}%", x * 100.0);
    let mb = |b: u64| format!("{:.0}", b as f64 / 1048576.0);
    let on = |m: u32| if sides[m as usize].enabled { "on" } else { "off" };
    let _ = writeln!(
        w,
        "# families: base first; usurpers — two of one family on where one should be"
    );
    let _ = writeln!(
        w,
        "family\trole\tmod\tstate\tMB\tlink\tto\tsure\tcover\tcover back\tsame bytes\twhy"
    );
    for (fi, f) in fams.iter().enumerate() {
        let mut order = f.members.clone();
        order.sort_by_key(|&m| {
            (
                m != f.root,
                !sides[m as usize].enabled,
                std::cmp::Reverse(sides[m as usize].bytes),
            )
        });
        for m in order {
            // this member's link toward the family (the one with the base, else its first)
            let l = f
                .links
                .iter()
                .find(|l| (l.a == m && l.b == f.root) || (l.b == m && l.a == f.root))
                .or_else(|| f.links.iter().find(|l| l.a == m || l.b == m));
            let role = if m == f.root { "base" } else { "" };
            let usurper = f.usurpers.iter().any(|&(a, b, _)| a == m || b == m);
            let (kind, to, sure, cab, cba, same, why) = match l {
                Some(l) => {
                    let other = if l.a == m { l.b } else { l.a };
                    let (cab, cba) = if l.a == m {
                        (l.cover_ab, l.cover_ba)
                    } else {
                        (l.cover_ba, l.cover_ab)
                    };
                    let dir = match (l.oriented, l.a == m, l.kind) {
                        (true, true, Kind::Version) => " (старее)",
                        (true, false, Kind::Version) => " (новее)",
                        (true, true, Kind::Addon | Kind::Translation) => " (к основе)",
                        (true, false, Kind::Addon | Kind::Translation) => " (основа)",
                        _ => "",
                    };
                    (
                        format!("{}{dir}", l.kind.label()),
                        sides[other as usize].name.clone(),
                        if l.sure { "yes" } else { "likely" },
                        pct(cab),
                        pct(cba),
                        l.same.map_or("—".into(), pct),
                        format!("{:?}", l.why),
                    )
                }
                None => Default::default(),
            };
            let _ = writeln!(
                w,
                "{}\t{}{}\t{}\t{}\t{}\t{kind}\t{to}\t{sure}\t{cab}\t{cba}\t{same}\t{why}",
                fi + 1,
                role,
                if usurper { " USURPER" } else { "" },
                sides[m as usize].name,
                on(m),
                mb(sides[m as usize].bytes),
            );
        }
    }
    let _ = writeln!(
        w,
        "\n# covers: not kin — one mod covers most of another with its own bytes"
    );
    let _ = writeln!(
        w,
        "covering\tstate\tMB\tcovered\tstate\tMB\tcovered share\tsame bytes"
    );
    for l in links.iter().filter(|l| l.kind == Kind::Covers) {
        let _ = writeln!(
            w,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            sides[l.a as usize].name,
            on(l.a),
            mb(sides[l.a as usize].bytes),
            sides[l.b as usize].name,
            on(l.b),
            mb(sides[l.b as usize].bytes),
            pct(l.cover_ba),
            l.same.map_or("—".into(), pct)
        );
    }
    w.flush().map_err(|e| e.to_string())?;

    let mut by_kind: FxHashMap<&str, u32> = FxHashMap::default();
    for l in &links {
        *by_kind.entry(l.kind.label()).or_default() += 1;
    }
    let mut kinds: Vec<_> = by_kind.into_iter().collect();
    kinds.sort();
    let usurped = fams.iter().filter(|f| !f.usurpers.is_empty()).count();
    println!(
        "kin: {} pairs looked at, {} links ({}), {} families of {} mods, {usurped} with a usurper; {} ms → {out}",
        pairs.len(),
        links.len(),
        kinds
            .iter()
            .map(|(k, n)| format!("{k} {n}"))
            .collect::<Vec<_>>()
            .join(", "),
        fams.len(),
        fams.iter().map(|f| f.members.len()).sum::<usize>(),
        t.elapsed().as_millis()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_game_files_make_the_surface() {
        assert!(game_file("textures/actors/a.dds"));
        assert!(game_file("Interface/foo.swf"));
        assert!(
            game_file("textures/preview.png"),
            "a picture in a folder is a game file"
        );
        assert!(!game_file("meta.ini"));
        assert!(!game_file("fomod/ModuleConfig.xml"));
        assert!(!game_file("Readme - Install.txt"));
        assert!(!game_file("screenshot.jpg"));
        assert!(!game_file("docs/changes.txt"));
    }
}
