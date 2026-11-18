//! Effects of the list in the window: read them, replace them, keep them with the project. @sat:app.effects.view
//!
//! The command `SetEffects` and the query `Effects` are `@sat:app.effects` in `lib.rs`; the state lives in the app
//! (`App::effects`, loaded from and saved with the project).
//! Owns: the tests of that round trip.
//! Not here: the effects (`@sat:effects`), drawing them (the window's effects block).

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use sat_core::canon::{Entry, EntryKind, Modlist};
    use sat_core::effects::{BadgeModel, Effects};
    use sat_core::order::OrderBook;
    use sat_core::resource::Registry;
    use sat_core::session::Session;
    use sat_core::table::ProviderTable;

    use crate::{App, Command, Event, Query, Reply};

    #[test]
    fn effects_are_kept_with_the_project_inside_their_bounds() {
        let list = Modlist::new(vec![Entry {
            name: "A".into(),
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
        a.load_project(&sat_core::project::Project::import(list), vec![]);
        let Reply::Effects { effects } = a.query(Query::Effects).unwrap() else {
            panic!("effects")
        };
        assert_eq!(effects, Effects::default());

        let mut e = Effects::default();
        e.indicator.size = 99;
        e.badges.model = BadgeModel::Summary;
        e.colour_focus.enabled = true;
        a.command(Command::SetEffects { effects: e.clone() }).unwrap();
        let saved = a.project().unwrap();
        assert_eq!(saved.effects.indicator.size, 16, "brought inside its bounds");
        assert_eq!(saved.effects.badges.model, BadgeModel::Summary);
        assert!(saved.effects.colour_focus.enabled);
        // the same effects again change nothing
        assert!(matches!(
            a.command(Command::SetEffects { effects: e }).unwrap(),
            Event::Nothing
        ));
    }
}
