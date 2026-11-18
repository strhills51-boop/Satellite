//! Focus groups: what a view can be pointed at (ADR 0009 «Фокус-группы вместо окна разделителей»). @sat:app.focus
//!
//! Owns: the built-in providers of focus groups — separator blocks, logical groups, tags, stickers, muted entries,
//! mods with plugins, and every build indicator's condition (a saved filter) — each group with its count and the
//! entries it holds; pointing a view at a group (the view's area, `Filter::area`) and back to the whole list.
//! Not here: the view's filter itself (`@sat:order.book.filter`), indicator definitions (`@sat:counters.build`).

use serde::{Deserialize, Serialize};

use sat_core::canon::EntryKind;
use sat_core::order::{Key, ViewId};

use crate::App;

/// Named sets of entries one provider gives.
type Groups = Vec<(String, Vec<Key>)>;

/// A focus group as the window lists it. `id` is `provider:name` and stable while the group lives.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FocusGroup {
    pub id: String,
    pub name: String,
    pub count: u32,
    pub enabled: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FocusProvider {
    pub id: String,
    pub title: String,
    pub groups: Vec<FocusGroup>,
}

impl App {
    /// Every provider with its groups and their entries (mods only), in canon order.
    fn focus_all(&mut self) -> Vec<(FocusProvider, Vec<Vec<Key>>)> {
        let book = self.session().book();
        let mods: Vec<(Key, String, bool)> = book
            .entries()
            .filter(|e| e.1 == EntryKind::Mod)
            .filter_map(|(n, _, on)| Some((book.key_of(n)?, n.to_string(), on)))
            .collect();
        let mut out: Vec<(FocusProvider, Vec<Vec<Key>>)> = Vec::new();
        let mut raw: Vec<(&str, &str, Groups)> = Vec::new();
        // blocks: the mods of each separator
        let mut blocks: Groups = Vec::new();
        {
            let b = self.session().book();
            let mut cur: Option<usize> = None;
            for (n, kind, _) in b.entries() {
                match kind {
                    EntryKind::Separator => {
                        blocks.push((
                            n.trim_end_matches(sat_core::canon::SEPARATOR_SUFFIX).to_string(),
                            Vec::new(),
                        ));
                        cur = Some(blocks.len() - 1);
                    }
                    EntryKind::Mod => {
                        if let (Some(i), Some(k)) = (cur, b.key_of(n)) {
                            blocks[i].1.push(k);
                        }
                    }
                    EntryKind::Foreign => {}
                }
            }
        }
        raw.push(("blocks", "Блоки", blocks));
        let groups = self
            .session()
            .book()
            .group_members()
            .into_iter()
            .map(|(n, _, ks)| (n, ks))
            .collect();
        raw.push(("groups", "Лог-группы", groups));
        // tags and stickers
        let (mut tags, mut stickers): (Groups, Groups) = (Vec::new(), Vec::new());
        {
            let b = self.session().book();
            for (k, _, _) in &mods {
                if let Some(m) = b.marks(*k) {
                    for t in &m.tags {
                        match tags.iter_mut().find(|x| x.0.eq_ignore_ascii_case(t)) {
                            Some(x) => x.1.push(*k),
                            None => tags.push((t.clone(), vec![*k])),
                        }
                    }
                    for st in &m.stickers {
                        match stickers.iter_mut().find(|x| &x.0 == st) {
                            Some(x) => x.1.push(*k),
                            None => stickers.push((st.clone(), vec![*k])),
                        }
                    }
                }
            }
        }
        tags.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(&b.0)));
        raw.push(("tags", "Теги", tags));
        raw.push(("stickers", "Стикеры", stickers));
        // plugins: mods that bring a plugin, and by what their plugins put on the indicator (`@sat:plugins.mod-mark`):
        // the functional focus groups of the plugin truth
        {
            use sat_core::plugins::Mark;
            let names: std::collections::HashMap<String, Key> =
                mods.iter().map(|(k, n, _)| (n.to_lowercase(), *k)).collect();
            let table = self.session().table().clone();
            let plugins = self.session().plugins();
            let marks = plugins.mod_marks(table.mod_count());
            let mut with: Vec<(Key, &sat_core::plugins::ModMark)> = table
                .mod_names()
                .iter()
                .zip(&marks)
                .filter(|(_, mk)| mk.plugins > 0)
                .filter_map(|(n, mk)| names.get(&n.to_lowercase()).map(|&k| (k, mk)))
                .collect();
            let b = self.session().book();
            with.sort_by_key(|(k, _)| b.pos(*k));
            let pick = |f: &dyn Fn(&sat_core::plugins::ModMark) -> bool| -> Vec<Key> {
                with.iter().filter(|(_, mk)| f(mk)).map(|(k, _)| *k).collect()
            };
            let has =
                |m: Mark| move |mk: &sat_core::plugins::ModMark| mk.top == Some(m) || mk.stack.contains(&m);
            let groups: Vec<(String, Vec<Key>)> = vec![
                ("с плагинами".into(), pick(&|_| true)),
                (
                    "выключенные с плагинами".into(),
                    with.iter()
                        .filter(|(k, _)| !b.enabled(*k))
                        .map(|(k, _)| *k)
                        .collect(),
                ),
                ("плагин без мастера".into(), pick(&has(Mark::Red))),
                ("переполнение".into(), pick(&has(Mark::Yellow))),
                ("необязательные патчи".into(), pick(&has(Mark::Optional))),
                ("выключенные плагины".into(), pick(&has(Mark::Unchecked))),
                ("копия перекрыта".into(), pick(&has(Mark::Overridden))),
                ("скрытые плагины".into(), pick(&has(Mark::Hidden))),
                ("esm".into(), pick(&|mk| mk.top == Some(Mark::Esm))),
                ("esp".into(), pick(&|mk| mk.top == Some(Mark::Esp))),
                ("esl".into(), pick(&|mk| mk.top == Some(Mark::Esl))),
            ];
            raw.push(("plugins", "Плагины", groups));
        }
        let muted: Vec<Key> = {
            let b = self.session().book();
            mods.iter().map(|m| m.0).filter(|&k| b.is_muted(k)).collect()
        };
        raw.push(("muted", "Приглушённое", vec![("приглушённые".into(), muted)]));
        // conflict contexts: the mods of each context (the heaviest first)
        {
            let ow = self.session.overwrite();
            let table = self.session.table().clone();
            let names: std::collections::HashMap<String, Key> =
                mods.iter().map(|(k, n, _)| (n.to_lowercase(), *k)).collect();
            let items = self.contexts();
            let mut cx: Groups = Vec::new();
            for (item, c) in items.iter().zip({
                let mut v: Vec<_> = ow.contexts().iter().collect();
                v.sort_by_key(|c| std::cmp::Reverse((c.files, c.mods.len())));
                v
            }) {
                let mut ks: Vec<Key> = c
                    .mods
                    .iter()
                    .filter_map(|&m| names.get(&table.mod_name(m).to_lowercase()).copied())
                    .collect();
                ks.sort_by_key(|&k| self.session.book().pos(k));
                cx.push((item.name.clone(), ks));
            }
            cx.truncate(40);
            raw.push(("contexts", "Конфликты", cx));
        }
        // SYNC: entities whose folder and project disagree, waiting for a decision
        {
            use sat_core::sync::records::{Mismatch, Status};
            let b = self.session.book();
            let mut ks: Vec<Key> = self
                .sync_records()
                .iter()
                .filter(|r| r.status == Status::Pending)
                .filter_map(|r| match &r.mismatch {
                    Mismatch::FsDeleted { project_name } | Mismatch::FsRenamed { project_name, .. } => {
                        b.key_of(project_name)
                    }
                    Mismatch::FsCreated { .. } => None,
                })
                .collect();
            ks.sort_by_key(|&k| b.pos(k));
            ks.dedup();
            raw.push(("sync", "SYNC", vec![("ждут решения".into(), ks)]));
        }
        // saved filters: every build indicator's condition
        let defs = if self.indicator_defs.is_empty() {
            sat_core::counters::default_indicators()
        } else {
            self.indicator_defs.clone()
        };
        let parts: Vec<_> = mods.iter().map(|m| self.view_parts(m.0)).collect();
        let mut filters = Vec::new();
        for d in defs.iter().filter(|d| !d.when.trim().is_empty()) {
            let Ok(e) = sat_core::counters::compile_expr(&d.when) else {
                continue;
            };
            let ks: Vec<Key> = mods
                .iter()
                .zip(&parts)
                .filter(|(_, p)| {
                    let v = p.view();
                    let get = |id: &str| v.get(id);
                    e.test(&get)
                })
                .map(|(m, _)| m.0)
                .collect();
            filters.push((d.label.clone(), ks));
        }
        raw.push(("filters", "Фильтры", filters));
        let b = self.session.book();
        for (id, title, groups) in raw {
            let groups: Groups = groups.into_iter().filter(|g| !g.1.is_empty()).collect();
            if groups.is_empty() {
                continue;
            }
            let views = groups
                .iter()
                .map(|(n, ks)| FocusGroup {
                    id: format!("{id}:{n}"),
                    name: n.clone(),
                    count: ks.len() as u32,
                    enabled: ks.iter().filter(|&&k| b.enabled(k)).count() as u32,
                })
                .collect();
            out.push((
                FocusProvider {
                    id: id.into(),
                    title: title.into(),
                    groups: views,
                },
                groups.into_iter().map(|g| g.1).collect(),
            ));
        }
        out
    }

    // @sat:app.focus.list
    pub(crate) fn focus_groups(&mut self) -> Vec<FocusProvider> {
        self.focus_all().into_iter().map(|p| p.0).collect()
    }

    // @sat:app.focus.point
    /// The filter that points view `v` at a focus group (its mods become the view's area), or back at the whole
    /// list (`None`); the rest of the view's filter stays.
    pub(crate) fn focus_filter(
        &mut self,
        v: ViewId,
        group: Option<&str>,
    ) -> Result<sat_core::order::Filter, crate::AppError> {
        let area = match group {
            None => None,
            Some(id) => Some(
                self.focus_all()
                    .into_iter()
                    .find_map(|(p, ks)| p.groups.iter().position(|g| g.id == id).map(|i| ks[i].clone()))
                    .ok_or_else(|| crate::AppError::Expr(format!("no focus group {id:?}")))?,
            ),
        };
        let mut f = self.session.book().filter(v).clone();
        f.area = area;
        Ok(f)
    }
}
