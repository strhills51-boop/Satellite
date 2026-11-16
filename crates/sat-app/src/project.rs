//! The app's truth and the project: what a project gives the app, what the app gives back. @sat:app.project
//!
//! Owns: the one mapping between the app's truth (book, settled chains, ignores, plugin canon, sync records, tags,
//! chain identities / layout / lineage, pins) and the sections of a [`Project`] — read when a build or a project
//! opens, written back before a run (the run reconciles the canon the user has now) and when the project is saved.
//! Not here: the project file (`@sat:stll.file`, `@sat:home.project`), the project's meaning (`@sat:project`).

use sat_core::project::Project;

use crate::App;

impl App {
    /// What the app takes from a project besides the canon (the session is built from its book): settled chains,
    /// member keys, conflict ignores, the plugin canon, sync records, tags, chain identities, layout and lineage,
    /// pins, stickers, sources, counters. Returns the problems of rules that did not compile.
    pub fn load_project(&mut self, p: &Project, disk: Vec<String>) -> Vec<String> {
        let session = self.session();
        session.set_member_keys(p.member_keys());
        session.set_decisions(p.decisions.clone());
        session.set_conflict_ignore(p.conflict_ignore.clone());
        session.set_plugin_canon(p.plugins.clone());
        self.set_sync(p.sync.clone(), disk);
        self.set_tag_catalog(p.tags.clone());
        self.set_chain_identities(p.chain_identities.clone());
        self.set_chain_layout(p.chain_layout.clone());
        self.set_chain_lineage(p.chain_lineage.clone());
        self.set_pinned_files(p.pinned_files.clone());
        let mut problems = self.set_sticker_rules(&p.sticker_rules);
        self.set_system_stickers(p.system_stickers());
        self.set_sources(p.sources_by_name());
        problems.extend(self.set_counters(&p.counters));
        problems
    }

    // @sat:app.project.store
    /// Writes the app's truth back into `p`: the canon and decoration of the book, settled chains, ignores, the
    /// plugin canon, sync records with their decisions, tags, chain identities, layout and lineage, pins. Sections
    /// the window does not edit (factories, patches, intents, rules, sources, facts) stay as `p` has them.
    pub fn store_project(&mut self, p: &mut Project) {
        let session = self.session();
        p.commit_book(session.book());
        p.decisions = session.decisions().clone();
        p.conflict_ignore = session.conflict_ignore().clone();
        p.plugins = session.plugin_canon().clone();
        p.sync = self.sync_records().to_vec();
        p.tags = self.tags.clone();
        p.chain_identities = self.chain_identities().to_vec();
        p.chain_layout = self.chain_layout().clone();
        p.chain_lineage = self.chain_lineage().clone();
        p.pinned_files = self.pinned_files().to_vec();
    }
}
