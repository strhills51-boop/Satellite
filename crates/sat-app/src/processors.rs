//! The processors of the project in the window (ADR 0021 step 4). @sat:app.processors
//!
//! Owns: the project's processor definitions as the window edits them (replace, refused when ids or outputs
//! repeat), taking in the processors the presets recognise among MO2's tools, and the view: each definition with
//! its state against the build (clean / dirty and why / no tool / off / cycle) and its inputs.
//! Not here: what is dirty and the run (`@sat:processor`, `@sat:home.plan.processors`), MO2's tools
//! (`@sat:mo2.executables`). A processor runs only in a commit (`--write`): the window never starts a tool.

use serde::{Deserialize, Serialize};

use sat_core::processor::{PRESETS, Processor, Status, detect};

use crate::{App, AppError};

/// A processor with its state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessorView {
    #[serde(flatten)]
    pub def: Processor,
    /// Its state against the build; `None` — the window has no build to assess it on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<Status>,
}

/// A preset the window offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetView {
    pub id: String,
    pub title: String,
    /// The MO2 tool it would run (recognised by its file name), when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
}

// @sat:app.processors.check
/// Whether a set of definitions can stand: ids, tools and outputs given, ids and outputs not repeated.
pub fn check(ps: &[Processor]) -> Result<(), String> {
    for (i, p) in ps.iter().enumerate() {
        if p.id.trim().is_empty() || p.executable.trim().is_empty() || p.output.trim().is_empty() {
            return Err(format!(
                "processor «{}»: an id, a tool and an output mod are needed",
                p.title
            ));
        }
        for q in &ps[..i] {
            if q.id == p.id {
                return Err(format!("two processors are called «{}»", p.id));
            }
            if q.output.eq_ignore_ascii_case(&p.output) {
                return Err(format!(
                    "«{}» and «{}» would write the same mod «{}»",
                    q.title, p.title, p.output
                ));
            }
        }
    }
    Ok(())
}

impl App {
    // @sat:app.processors.set
    /// Replaces the project's processors. A definition changed here keeps its last fingerprint: changing what it
    /// reads or writes makes it dirty by itself (the fingerprint covers the definition).
    pub fn set_processors(&mut self, ps: Vec<Processor>) -> Result<(), AppError> {
        check(&ps).map_err(AppError::Expr)?;
        self.processors = ps;
        self.revision += 1;
        Ok(())
    }

    // @sat:app.processors.detect
    /// Adds the processors the presets recognise among MO2's tools (turned off); returns how many.
    pub fn detect_processors(&mut self) -> usize {
        let tools = self.processor_tools();
        let found = detect(&tools, &self.processors);
        let n = found.len();
        if n > 0 {
            self.processors.extend(found);
            self.revision += 1;
        }
        n
    }

    fn processor_tools(&mut self) -> Vec<(String, String)> {
        match (self.plan_source.clone(), self.project()) {
            (Some(src), Some(p)) => src.processors(&p).1,
            _ => Vec::new(),
        }
    }

    // @sat:app.processors.view
    /// The processors with their states, MO2's tools and the presets (with the tool each would run).
    pub(crate) fn processors_view(&mut self) -> (Vec<ProcessorView>, Vec<String>, Vec<PresetView>) {
        let (states, tools) = match (self.plan_source.clone(), self.project()) {
            (Some(src), Some(p)) => src.processors(&p),
            _ => (Vec::new(), Vec::new()),
        };
        let views = self
            .processors
            .iter()
            .map(|d| ProcessorView {
                def: d.clone(),
                status: states.iter().find(|s| s.id == d.id).cloned(),
            })
            .collect();
        let presets = PRESETS
            .iter()
            .map(|p| PresetView {
                id: p.id.into(),
                title: p.title.into(),
                tool: tools
                    .iter()
                    .find(|(_, bin)| p.binaries.iter().any(|b| bin.contains(b)))
                    .map(|(t, _)| t.clone()),
            })
            .collect();
        (views, tools.into_iter().map(|(t, _)| t).collect(), presets)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use sat_core::canon::{Entry, EntryKind, Modlist};
    use sat_core::order::OrderBook;
    use sat_core::processor::{Dirty, State};
    use sat_core::session::Session;
    use sat_core::table::ProviderTable;

    use crate::{App, Command, Query, Reply};
    use sat_core::resource::Registry;

    #[derive(Debug)]
    struct Shell;
    impl crate::PlanSource for Shell {
        fn full_plan(
            &self,
            project: &sat_core::project::Project,
            _: &[String],
        ) -> (sat_core::sync::plan::Plan, crate::PlanExtras) {
            (
                sat_core::sync::plan::plan(&project.canon, &project.sync, &[]),
                crate::PlanExtras::default(),
            )
        }
        fn processors(
            &self,
            project: &sat_core::project::Project,
        ) -> (Vec<sat_core::processor::Status>, Vec<(String, String)>) {
            let st = project
                .processors
                .iter()
                .map(|p| sat_core::processor::Status {
                    id: p.id.clone(),
                    state: if p.enabled {
                        State::Dirty {
                            why: Dirty::NeverBuilt,
                        }
                    } else {
                        State::Off
                    },
                    inputs: vec!["Animations".into()],
                    fingerprint: "f".into(),
                })
                .collect();
            (
                st,
                vec![
                    ("Pandora".into(), "pandora behaviour engine+.exe".into()),
                    ("Notepad".into(), "notepad.exe".into()),
                ],
            )
        }
    }

    #[test]
    fn the_window_takes_in_recognised_tools_and_edits_processors() {
        let list = Modlist::new(vec![Entry {
            name: "Animations".into(),
            kind: EntryKind::Mod,
            enabled: true,
        }]);
        let mut a = App::new(
            Session::new(
                OrderBook::from_modlist(&list),
                Arc::new(ProviderTable::build(&[])),
            ),
            Registry::with_builtins(),
        );
        a.load_project(
            &sat_core::project::Project::import(list),
            vec!["Animations".into()],
        );
        a.set_plan_source(Arc::new(Shell));
        a.command(Command::DetectProcessors).unwrap();
        let Reply::Processors {
            processors,
            tools,
            presets,
        } = a.query(Query::Processors).unwrap()
        else {
            panic!()
        };
        assert_eq!(processors.len(), 1, "only Pandora is recognised");
        assert_eq!(processors[0].def.executable, "Pandora");
        assert!(!processors[0].def.enabled, "a found processor starts off");
        assert_eq!(processors[0].status.as_ref().unwrap().state, State::Off);
        assert_eq!(tools, ["Pandora", "Notepad"]);
        assert!(
            presets
                .iter()
                .any(|p| p.id == "pandora" && p.tool.as_deref() == Some("Pandora"))
        );
        // turned on: dirty, and it is in the project the commit sees
        let mut ps: Vec<_> = processors.into_iter().map(|v| v.def).collect();
        ps[0].enabled = true;
        a.command(Command::SetProcessors {
            processors: ps.clone(),
        })
        .unwrap();
        let Reply::Processors { processors, .. } = a.query(Query::Processors).unwrap() else {
            panic!()
        };
        assert!(matches!(
            processors[0].status.as_ref().unwrap().state,
            State::Dirty { .. }
        ));
        assert!(a.project().unwrap().processors[0].enabled);
        // two processors writing one mod are refused
        let mut twice = ps.clone();
        twice.push(sat_core::processor::Processor {
            id: "other".into(),
            ..ps[0].clone()
        });
        assert!(a.command(Command::SetProcessors { processors: twice }).is_err());
    }
}
