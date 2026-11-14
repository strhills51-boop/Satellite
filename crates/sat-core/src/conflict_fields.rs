//! Record fields: who changed what inside a conflicting record, and what the winner loses. @sat:conflict.fields
//!
//! ADR 0017 §1, ADR 0018 §5.4.
//!
//! Owns: the fields of a record as one plugin has it (`Fields`: signature and hash per subrecord, references
//! mapped to their plugins), and the classification of every record conflict by field provenance:
//! - **covered** — the winner carries every other plugin's changes (nothing is lost);
//! - **mergeable** — changes are lost, but no field is changed by two plugins in different ways (a patch can be
//!   built without a person; repeated fields — inventories, leveled entries — are collections, their result a union);
//! - **real** — one field changed by two plugins in different ways: someone has to choose.
//!
//! Not here: reading fields (`@sat:scan.records.fields`), the record index (`@sat:records`).
//!
//! The base of a record is its own plugin's copy; when that plugin is not loaded the first copy stands for it (and
//! its own changes cannot be told). Fields compare by bytes with references mapped through each plugin's masters;
//! a field whose layout hides a reference elsewhere may read as changed when it is not — never the other way.

use rustc_hash::FxHashMap;

use crate::conflict::{
    Builder, Centrality, DomainInfo, DomainReport, Family, Origin, Semantics, Severity, cell,
};
use crate::records::RecordIndex;

/// One record's fields in one plugin: (signature, hash), in record order.
pub type Fields = Vec<([u8; 4], u64)>;

/// Fields of the conflict records, by (plugin load index, record key).
#[derive(Debug, Default, Clone)]
pub struct FieldFacts(pub FxHashMap<(u32, u64), Fields>);

/// A record's fields by signature: a value (the first copy) or, for a field that repeats, a multiset.
#[derive(Default)]
struct Slots {
    values: FxHashMap<[u8; 4], Vec<u64>>,
}

impl Slots {
    fn of(f: &Fields) -> Self {
        let mut s = Slots::default();
        for (sig, h) in f {
            s.values.entry(*sig).or_default().push(*h);
        }
        for v in s.values.values_mut() {
            v.sort_unstable();
        }
        s
    }

    fn get(&self, sig: &[u8; 4]) -> &[u64] {
        self.values.get(sig).map_or(&[], Vec::as_slice)
    }
}

/// Elements of `a` not in `b` (multisets, sorted).
fn minus(a: &[u64], b: &[u64]) -> Vec<u64> {
    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < a.len() {
        if j < b.len() && a[i] == b[j] {
            i += 1;
            j += 1;
        } else if j < b.len() && b[j] < a[i] {
            j += 1;
        } else {
            out.push(a[i]);
            i += 1;
        }
    }
    out
}

/// What a record conflict is, by its fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldVerdict {
    Covered,
    Mergeable,
    Real,
}

// @sat:conflict.fields.classify
/// Classifies one record: per provider (load order) its slots; `base` — the index of the base copy. Returns the
/// verdict, per provider whether it loses a change, and the lost fields.
fn classify_one(
    copies: &[Slots],
    base: usize,
    collections: &[[u8; 4]],
) -> (FieldVerdict, Vec<bool>, Vec<[u8; 4]>) {
    let w = copies.len() - 1;
    let mut sigs: Vec<[u8; 4]> = copies.iter().flat_map(|c| c.values.keys().copied()).collect();
    sigs.sort_unstable();
    sigs.dedup();
    let is_collection = |s: &[u8; 4]| collections.contains(s) || copies.iter().any(|c| c.get(s).len() > 1);
    let mut loses = vec![false; copies.len()];
    let mut lost: Vec<[u8; 4]> = Vec::new();
    let mut real = false;
    for s in &sigs {
        let b = copies[base].get(s);
        let win = copies[w].get(s);
        if is_collection(s) {
            for (i, c) in copies.iter().enumerate() {
                if i == base || i == w {
                    continue;
                }
                let v = c.get(s);
                let added = minus(v, b);
                let removed = minus(b, v);
                // an addition the winner lacks, a removal the winner still has
                let lost_here = !minus(&added, win).is_empty() || removed.iter().any(|r| win.contains(r));
                if lost_here {
                    loses[i] = true;
                    lost.push(*s);
                }
            }
            continue;
        }
        // a value: who changed it, and to what
        let changed: Vec<(usize, &[u64])> = copies
            .iter()
            .enumerate()
            .filter(|&(i, c)| i != base && c.get(s) != b)
            .map(|(i, c)| (i, c.get(s)))
            .collect();
        for &(i, v) in &changed {
            if i != w && v != win {
                loses[i] = true;
                lost.push(*s);
            }
        }
        let mut values: Vec<&[u64]> = changed.iter().map(|x| x.1).collect();
        values.sort_unstable();
        values.dedup();
        if values.len() > 1 && changed.iter().any(|&(i, v)| i != w && v != win) {
            real = true;
        }
    }
    lost.sort_unstable();
    lost.dedup();
    let verdict = if lost.is_empty() {
        FieldVerdict::Covered
    } else if real {
        FieldVerdict::Real
    } else {
        FieldVerdict::Mergeable
    };
    (verdict, loses, lost)
}

// @sat:conflict.fields.report
/// Every record conflict, classified by its fields. Records whose copies were not all read are left out and counted
/// in the notes.
pub fn classify(ix: &RecordIndex, facts: &FieldFacts, collections: &[[u8; 4]]) -> DomainReport {
    let mut b = Builder::new(DomainInfo {
        id: "fields".into(),
        title: "Поля записей".into(),
        about: "Конфликт записей по полям: «покрыт» — победитель несёт правки всех; «сливаемый» — правки теряются, но поля не пересекаются (патч собирается без человека, списки сливаются); «настоящий» — одно поле разные плагины меняют по-разному.".into(),
        family: Family::Merge,
        centrality: Centrality::PluginOrder,
        semantics: Semantics::Mergeable,
        origin: Origin::Builtin,
    });
    let mut unread = 0u32;
    for (key, plugins) in ix.field_targets() {
        let Some(copies) = plugins
            .iter()
            .map(|&p| facts.0.get(&(p, key)).map(Slots::of))
            .collect::<Option<Vec<Slots>>>()
        else {
            unread += 1;
            continue;
        };
        let provs = ix.providers(key);
        let base = 0;
        let has_origin = provs.first().is_some_and(|p| p.origin);
        let (verdict, loses, lost) = classify_one(&copies, base, collections);
        let w = plugins.len() - 1;
        let members: Vec<(u32, u8)> = plugins
            .iter()
            .enumerate()
            .map(|(i, &p)| {
                let id = b.participant(&ix.plugins[p as usize], None, i64::from(p));
                let c = if i == w {
                    cell::WIN
                } else if i == base && has_origin {
                    cell::ORIGIN
                } else if loses[i] {
                    cell::LOSE
                } else {
                    cell::SAME
                };
                (id, c)
            })
            .collect();
        let sig = provs
            .last()
            .map(|p| String::from_utf8_lossy(&p.signature).into_owned())
            .unwrap_or_default();
        let fields: Vec<String> = lost
            .iter()
            .take(6)
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect();
        let target = if fields.is_empty() {
            format!("{} {sig}", ix.label(key))
        } else {
            format!(
                "{} {sig} · {}{}",
                ix.label(key),
                fields.join(" "),
                if lost.len() > 6 { " …" } else { "" }
            )
        };
        let (kind, sev, unresolved) = match verdict {
            FieldVerdict::Covered => ("покрыт", Severity::Info, false),
            FieldVerdict::Mergeable => ("сливаемый", Severity::Warn, true),
            FieldVerdict::Real => ("настоящий", Severity::Warn, true),
        };
        b.contest(&target, &members, kind, sev, unresolved);
    }
    if unread > 0 {
        b.note(format!(
            "{unread} записей не прочитаны по полям (плагин не открылся или запись не распаковалась)"
        ));
    }
    let mut r = b.finish();
    // real before mergeable within the same severity
    r.chains.sort_by(|a, c| {
        c.severity
            .cmp(&a.severity)
            .then((c.kind == "настоящий").cmp(&(a.kind == "настоящий")))
            .then(c.unresolved.cmp(&a.unresolved))
            .then(c.targets.len().cmp(&a.targets.len()))
    });
    let ps = r.participants.clone();
    r.contexts = crate::conflict::contexts_of(&ps, &r.chains);
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slots(f: &[(&[u8; 4], u64)]) -> Slots {
        Slots::of(&f.iter().map(|(s, h)| (**s, *h)).collect())
    }

    #[test]
    fn covered_mergeable_and_real_by_who_changed_which_field() {
        let base = slots(&[(b"EDID", 1), (b"DATA", 10), (b"AIDT", 20), (b"CNTO", 100)]);
        // A changes AIDT, B (winner) changes DATA and keeps A's AIDT: covered
        let a = slots(&[(b"EDID", 1), (b"DATA", 10), (b"AIDT", 21), (b"CNTO", 100)]);
        let w = slots(&[(b"EDID", 1), (b"DATA", 11), (b"AIDT", 21), (b"CNTO", 100)]);
        let (v, loses, _) = classify_one(&[base, a, w], 0, &[]);
        assert_eq!((v, loses), (FieldVerdict::Covered, vec![false, false, false]));

        // the winner does not carry A's AIDT, nobody else touched it: mergeable; A's inventory addition is lost too
        let base = slots(&[(b"DATA", 10), (b"AIDT", 20), (b"CNTO", 100), (b"CNTO", 101)]);
        let a = slots(&[
            (b"DATA", 10),
            (b"AIDT", 21),
            (b"CNTO", 100),
            (b"CNTO", 101),
            (b"CNTO", 102),
        ]);
        let w = slots(&[(b"DATA", 11), (b"AIDT", 20), (b"CNTO", 100), (b"CNTO", 101)]);
        let (v, loses, lost) = classify_one(&[base, a, w], 0, &[]);
        assert_eq!(v, FieldVerdict::Mergeable);
        assert_eq!(loses, vec![false, true, false]);
        assert_eq!(lost, vec![*b"AIDT", *b"CNTO"]);

        // both change DATA differently: real
        let base = slots(&[(b"DATA", 10)]);
        let a = slots(&[(b"DATA", 12)]);
        let w = slots(&[(b"DATA", 11)]);
        let (v, _, lost) = classify_one(&[base, a, w], 0, &[]);
        assert_eq!((v, lost), (FieldVerdict::Real, vec![*b"DATA"]));
    }

    #[test]
    fn the_report_names_lost_fields_and_marks_the_winner_and_the_source() {
        use crate::records::{PluginRecords, RecordFact};
        let rec = |h: u64| RecordFact {
            form_id: 0x0000_0010,
            signature: *b"NPC_",
            hash: Some(h),
            flags: 0,
        };
        let base = PluginRecords {
            masters: vec![],
            records: vec![rec(1)],
        };
        let a = PluginRecords {
            masters: vec!["Skyrim.esm".into()],
            records: vec![rec(2)],
        };
        let w = PluginRecords {
            masters: vec!["Skyrim.esm".into()],
            records: vec![rec(3)],
        };
        let ix = RecordIndex::build(&[
            ("Skyrim.esm".into(), &base),
            ("A.esp".into(), &a),
            ("W.esp".into(), &w),
        ]);
        let key = ix.field_targets()[0].0;
        let mut f = FieldFacts::default();
        f.0.insert((0, key), vec![(*b"AIDT", 20)]);
        f.0.insert((1, key), vec![(*b"AIDT", 21)]);
        f.0.insert((2, key), vec![(*b"AIDT", 20), (*b"DATA", 5)]);
        let r = classify(&ix, &f, &[]);
        assert_eq!(r.chains[0].kind, "сливаемый");
        assert_eq!(r.chains[0].cells, b"olw");
        assert_eq!(r.targets[0], "skyrim.esm:000010 NPC_ · AIDT");
        assert_eq!(ix.raw_form_id(1, key), Some(0x0000_0010));
        assert_eq!(ix.raw_form_id(0, key), Some(0x0000_0010));
    }
}
