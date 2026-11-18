//! Forks of taste in the window: the list and the owner's choice. @sat:app.forks.view
//!
//! The command `ChooseFork` and the query `Forks` are `@sat:app.forks` in `lib.rs`.
//! Owns: the tests of the window's forks; the state lives in the app (`App::forks`, loaded from and saved with the
//! project) and the choice is a command in `lib.rs`.
//! Not here: the fork (`@sat:forks`), the CLI (`@sat:cli.forks`).

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use sat_core::canon::{Entry, EntryKind, Modlist};
    use sat_core::forks::{ChosenBy, Fork, ForkOption};
    use sat_core::order::OrderBook;
    use sat_core::resource::Registry;
    use sat_core::session::Session;
    use sat_core::table::ProviderTable;

    use crate::{App, Command, Query, Reply};

    #[test]
    fn the_owner_chooses_a_fork_in_the_window_and_the_list_follows() {
        let list = Modlist::new(
            ["Head A", "Head B", "Other"]
                .iter()
                .map(|n| Entry {
                    name: n.to_string(),
                    kind: EntryKind::Mod,
                    enabled: *n != "Head B",
                })
                .collect(),
        );
        let mut a = App::new(
            Session::new(
                OrderBook::from_modlist(&list),
                Arc::new(ProviderTable::build(&[])),
            ),
            Registry::with_builtins(),
        );
        let mut p = sat_core::project::Project::import(list);
        p.forks.push(Fork {
            id: "f-1".into(),
            aspect: "голова".into(),
            about: String::new(),
            options: ["Head A", "Head B"]
                .iter()
                .map(|n| ForkOption {
                    name: n.to_string(),
                    mods: vec![n.to_string()],
                    facts: String::new(),
                })
                .collect(),
            chosen: Some(0),
            by: ChosenBy::Claude,
            why: "default".into(),
        });
        a.load_project(&p, vec![]);
        let Reply::Forks { forks } = a.query(Query::Forks).unwrap() else {
            panic!("forks")
        };
        assert_eq!(forks.len(), 1);
        a.command(Command::ChooseFork {
            id: "f-1".into(),
            option: 1,
        })
        .unwrap();
        let saved = a.project().unwrap();
        let on: Vec<(&str, bool)> = saved
            .canon
            .entries
            .iter()
            .map(|e| (e.name.as_str(), e.enabled))
            .collect();
        assert_eq!(on, [("Head A", false), ("Head B", true), ("Other", true)]);
        assert_eq!(
            (saved.forks[0].chosen, saved.forks[0].by),
            (Some(1), ChosenBy::Owner)
        );
        assert!(
            a.command(Command::ChooseFork {
                id: "f-9".into(),
                option: 0
            })
            .is_err()
        );
    }
}
