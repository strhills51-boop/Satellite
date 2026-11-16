//! Application API: commands, queries and events over one session, independent of transport. @sat:app
//!
//! Owns: the message vocabulary every shell speaks (ADR 0004) — [`Command`] (intent to change), [`Query`] (read a
//! projection window), [`Event`] (what changed) — and the dispatcher that runs them on a [`Session`].
//! Not here: transports (`apps/devserver`, future desktop / web worker), loading a build from disk (shells),
//! the operations themselves (`@sat:session.command`, `@sat:order.book`).
//!
//! Rules (ADR 0004 §4–5): a command never returns rows, it returns an [`Event`] with row patches per view and the
//! new revision; the UI re-reads only the window it shows. Queries never change anything.

use std::sync::Arc;

mod advisor;
mod cards;
mod chains;
pub mod commit;
mod context;
mod domains;
pub mod find;
mod participants;
pub mod pass;
pub mod patches;
mod preview;
pub mod project;
pub mod records;
pub mod sheet;
mod sync;
mod tree;
pub use advisor::{AdviceRow, OrderAdvice};
pub use cards::{CardMember, ChainCard, ChainCardDetail};
pub use context::{ColumnOrder, ContextItem, ContextTable, TableChain, TableCol, TableRow};
pub use preview::{Asset, FileCopy, FilePreview, FileSource, PinnedFile, PlaceFile, VfsEntry};
pub use tree::{ChainRef, FileRow, FileTree, PathOwners, TreeSubject};

fn yes() -> bool {
    true
}

fn thumb_side() -> u32 {
    128
}

fn tree_limit() -> usize {
    2000
}
pub use domains::{DomainContextItem, DomainItem, DomainState, FileReader};
pub use sync::{Decision, PlanExtras, PlanSource, RecordView};

use serde::{Deserialize, Serialize};

use sat_core::ids::ModId;
use sat_core::order::flow::{FlowIndex, FlowParams};
use sat_core::order::{
    self, CardStyle, Filter, Key, MAIN, Nav, OrderError, RowKind, Select, Shift, ViewChange, ViewId,
};
use sat_core::overwrite::OverwriteStats;
use sat_core::plugins::PluginStats;
use sat_core::resource::{Chord, CommandError, Registry};
use sat_core::session::Session;

// @sat:app.command
/// An intent to change state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Command {
    /// A key press in a list window: looked up in the key map, run as a command resource.
    Key {
        view: ViewId,
        chord: String,
    },
    /// A command resource by id (menu, palette).
    Run {
        view: ViewId,
        id: String,
    },
    Navigate {
        view: ViewId,
        nav: Nav,
        select: Select,
    },
    ShiftSelection {
        view: ViewId,
        how: Shift,
    },
    /// Puts mods in this relative order among their own places, separator by separator (the order advisor's
    /// plan, `@sat:app.reorder`); one undo step.
    Reorder {
        keys: Vec<Key>,
    },
    Move {
        keys: Vec<Key>,
        before: Option<Key>,
        /// Or: just after this entry (the conflict table drops a mod below a row without knowing what follows it).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after: Option<Key>,
    },
    SetEnabled {
        keys: Vec<Key>,
        enabled: Option<bool>,
    },
    SetColor {
        keys: Vec<Key>,
        color: Option<u32>,
    },
    /// Nesting level of separators (ADR 0016): a separator holds the following separators of a higher level.
    SetLevel {
        keys: Vec<Key>,
        level: u8,
    },
    /// Card style of entries (ADR 0012); `None` clears it.
    SetStyle {
        keys: Vec<Key>,
        style: Option<CardStyle>,
    },
    /// Changes only the given style fields on every entry (JSON merge patch, RFC 7386: `null` clears a field);
    /// the other fields of each entry stay. One undo step.
    PatchStyle {
        keys: Vec<Key>,
        patch: serde_json::Value,
    },
    SetCollapsed {
        view: ViewId,
        separator: Key,
        collapsed: bool,
    },
    SetFilter {
        view: ViewId,
        filter: Filter,
    },
    /// Move the focus to an entry (a link from another block: "go to this mod").
    Focus {
        view: ViewId,
        key: Key,
    },
    /// Mark a chain settled ("done") or not, by its content signature (hex string: JSON numbers lose 64-bit
    /// precision).
    SetChainDone {
        signature: String,
        done: bool,
    },
    /// Check or uncheck a plugin; checking a plugin whose mods are all disabled enables its winning mod (monolith).
    SetPluginChecked {
        name: String,
        checked: bool,
    },
    /// Replace the build indicators (`@sat:app.indicators`); a definition that does not compile is refused.
    SetIndicators {
        defs: Vec<sat_core::counters::IndicatorDef>,
    },
    /// Change the buffer (`@sat:app.buffer`): areas of mods set aside to be placed later. One undo step each (placing
    /// is two: the move, then the area gone).
    Buffer {
        edit: BufferEdit,
    },
    /// Join contiguous mods into a logical group (`@sat:app.groups`): it starts collapsed — one row, one result.
    CreateGroup {
        name: String,
        keys: Vec<Key>,
    },
    /// Dissolve the logical group of an entry; its members stay where they are.
    DissolveGroup {
        key: Key,
    },
    /// Fold a logical group into one row, or open it: its members in the list, outlined as a family.
    SetGroupCollapsed {
        view: ViewId,
        key: Key,
        collapsed: bool,
    },
    /// Add, change or remove an element without an indicator after an entry (`@sat:app.attached`). One undo step.
    Attach {
        key: Key,
        edit: AttachEdit,
    },
    /// Edit tags, stickers or the note of entries; each entry keeps its other marks. One undo step.
    EditMarks {
        keys: Vec<Key>,
        edit: MarksEdit,
    },
    /// Replace the conflict ignore rules (`meta.ini`, `docs/`, `a/b.dds`): conflicts and chains are recomputed.
    SetConflictIgnore {
        rules: Vec<String>,
    },
    /// Hide a plugin (blacklist) or show it again.
    SetPluginHidden {
        name: String,
        hidden: bool,
    },
    /// Check plugins in layers by masters from the base game up; uncheck the rest of the present ones.
    CascadePlugins,
    /// Name, icon or colour a normalized chain (by signature); `None` everywhere takes the generated identity back.
    /// The identity follows the chain when it is recomputed (`@sat:naming.inherit`).
    SetChainIdentity {
        signature: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        emoji: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hue: Option<u16>,
    },
    /// Move plugins (in their current order) to just before `before`, or to the end; refused when the move breaks
    /// the game's order rules (`@sat:plugins.edit.order`).
    /// Merges cards (normalized chains by signature) into one (`@sat:app.layout`).
    MergeChains {
        signatures: Vec<String>,
    },
    /// Forgets the user's records no chain answers to any more (`@sat:app.orphans`).
    PruneOrphans,
    /// Pins or unpins a file for quick access (`@sat:app.preview`).
    PinFile {
        path: String,
        pinned: bool,
    },
    /// Splits a card into its exact chains.
    SplitChain {
        signature: String,
    },
    /// Gives a card's exact chains back to the system's grouping.
    ResetChain {
        signature: String,
    },
    MovePlugins {
        names: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before: Option<String>,
    },
    /// Run over the build (`@sat:app.pass`): the shell takes the request; a newer one supersedes a running run.
    StartPass {
        mode: pass::PassMode,
    },
    /// Stop the running run: nothing of it is published.
    CancelPass,
    /// Edit the patch of a mod's ini file (`@sat:app.patches`): the commit writes the patched copy into Satellite
    /// Patches; the mod's file is never touched.
    PatchFile {
        mod_name: String,
        path: String,
        #[serde(default)]
        edit: patches::PatchEdit,
    },
    /// Commit the plan to the build (`@sat:app.commit`; only when the shell writes): `confirmed` names steps not
    /// undone by a later commit (sealing a group).
    Commit {
        #[serde(default)]
        confirmed: Vec<String>,
    },
    /// Save the project (`@sat:app.project`; only when the shell writes): `force` overwrites a file changed outside.
    SaveProject {
        #[serde(default)]
        force: bool,
    },
    /// Decide a sync record (by fingerprint): pending, ignore, or confirm with an answer that fits it.
    Decide {
        fingerprint: String,
        decision: Decision,
    },
    Undo,
    Redo,
    OpenView,
    CloseView {
        view: ViewId,
    },
}

// @sat:app.query
/// A tag in use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagView {
    pub name: String,
    pub entries: usize,
    pub color: u32,
}

/// One change of the buffer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case")]
pub enum BufferEdit {
    /// Put entries into an area (`None` — a new area at the end); an entry already in another area moves here.
    Put {
        keys: Vec<Key>,
        area: Option<usize>,
    },
    /// Take an entry out of the buffer (it stays in the list where it is).
    Remove {
        key: Key,
    },
    /// Forget an area.
    Clear {
        area: usize,
    },
    Rename {
        area: usize,
        name: String,
    },
    /// Place an area's mods before `before` (`None` — the end of the list) in the area's order; the area goes.
    Place {
        area: usize,
        before: Option<Key>,
    },
}

/// The buffer as the window shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BufferAreaView {
    pub name: String,
    pub members: Vec<BufferMember>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BufferMember {
    pub key: Key,
    pub name: String,
    pub enabled: bool,
}

/// The permanent «Выделение» block (ADR 0020 §7): what is selected and what it adds up to.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectionView {
    pub count: u32,
    pub enabled: u32,
    /// The first names, in canon order (the rest is `count - names.len()`).
    pub names: Vec<(Key, String)>,
    /// Tags and stickers on the selection, with how many selected mods carry each.
    pub tags: Vec<(String, u32)>,
    pub stickers: Vec<(String, u32)>,
    /// Sums of the counters.
    pub sums: Counters,
}

/// One change of the elements after an entry (`@sat:order.decor.attached`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case")]
pub enum AttachEdit {
    /// A new element at the end of the entry's elements; the app gives it the next free id.
    Add {
        #[serde(flatten)]
        kind: sat_core::order::AttachKind,
        #[serde(default)]
        width: Option<u16>,
        #[serde(default)]
        height: Option<u16>,
    },
    /// Replaces the element with the same id (content, size, line breaks).
    Update {
        attachment: sat_core::order::Attachment,
    },
    Remove {
        id: u32,
    },
}

/// An element without an indicator as the window draws it: the element and, for a family summary, its counts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachView {
    pub attachment: sat_core::order::Attachment,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<FamilyCount>,
}

/// Entries carrying a tag and how many of them are on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FamilyCount {
    pub total: u32,
    pub enabled: u32,
}

/// One change of marks (`@sat:order.decor.marks`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case")]
pub enum MarksEdit {
    AddTag { tag: String },
    RemoveTag { tag: String },
    AddSticker { sticker: String },
    RemoveSticker { sticker: String },
    SetNote { note: String },
}

/// A read of a projection window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Query {
    /// Rows `[start, start + count)` of a view, with the mod card counters.
    Rows {
        view: ViewId,
        start: usize,
        count: usize,
        /// Chain icons per context badge on mod cards (the "density"); 0 = no badges.
        #[serde(default)]
        badges: u8,
    },
    Totals,
    /// The plugin list (full; a few thousand entries).
    Plugins,
    /// Active key bindings of the list context.
    Keymap,
    /// Everything the summary shows about one mod: with whom and in what it conflicts, its plugins.
    ModDetail {
        key: Key,
    },
    /// The inside of the logical group holding `key` (`@sat:session.group-inside`): its own conflicts.
    GroupInside {
        key: Key,
    },
    /// Mods an expression holds for (`@sat:app.find`): `size > 1 GB && plugins > 0`.
    Find {
        expr: String,
        limit: usize,
    },
    /// The standard quick look at a mod (`@sat:app.sheet`): a badge, a mini sheet or the full one.
    ModSheet {
        key: Key,
        level: sheet::SheetLevel,
    },
    /// Separator tiles of the left column.
    Separators,
    /// Tags in use with how many entries carry each.
    Tags,
    /// Sync records of the last analysis with their answers, and the commit plan they make now.
    Sync,
    /// The run: state, progress, the last result (`@sat:app.pass`).
    Pass,
    /// Saving the project: writable, unsaved changes, the last error (`@sat:app.project.state`).
    Project,
    /// The commit: state and the last result (`@sat:app.commit`).
    Commit,
    /// A mod's ini file with its patch: keys, the mod's and the patched values (`@sat:app.patches.view`).
    FilePatch {
        mod_name: String,
        path: String,
    },
    /// Conflict contexts for the table's selector, largest first.
    Contexts,
    /// A page of record chains (`@sat:app.records.page`), largest first; with `plugin` only its chains.
    RecordChains {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        plugin: Option<String>,
        /// Only conflicts (two or more plugins over a record), no single overrides.
        #[serde(default)]
        conflicts_only: bool,
        #[serde(default)]
        offset: usize,
        limit: usize,
    },
    /// Record conflict contexts (`@sat:app.records.contexts`), largest first.
    RecordContexts,
    /// A window of a record context's conflict table (`@sat:app.records.table`), in the file table's shape.
    RecordTable {
        context: usize,
        row_start: usize,
        row_count: usize,
        col_start: usize,
        col_count: usize,
    },
    /// Every conflict domain, files and records first (`@sat:app.domains.list`).
    ConflictDomains,
    /// The targets of one stack (`@sat:app.domains.stack`): of chain `chain` of context `context` of `domain`
    /// (files: the context of mod `key`), a window.
    StackTargets {
        domain: String,
        #[serde(default)]
        key: Option<Key>,
        #[serde(default)]
        context: usize,
        chain: usize,
        stack: usize,
        offset: usize,
        limit: usize,
    },
    /// Contexts of a domain other than files and records (`@sat:app.domains.contexts`).
    DomainContexts {
        domain: String,
    },
    /// Chain cards of a context of a domain (`@sat:app.domains.cards`).
    DomainCards {
        domain: String,
        context: usize,
    },
    /// Details of chain card `index`: members with teeth, the first `limit` targets.
    DomainCard {
        domain: String,
        context: usize,
        index: usize,
        limit: usize,
    },
    /// A window of a domain's context table (`@sat:app.domains.table`).
    DomainTable {
        domain: String,
        context: usize,
        row_start: usize,
        row_count: usize,
        col_start: usize,
        col_count: usize,
    },
    /// Chain cards of the file context of mod `key` (`@sat:app.cards.files`).
    ContextCards {
        key: Key,
        /// The chain settled last (its signature): the solution front picks the next card near it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        last: Option<String>,
        /// Chains put off for now (signatures): the front passes them by as if settled.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        skip: Vec<String>,
    },
    /// Details of chain card `index` of the file context of mod `key`: members with teeth, the first `limit` paths.
    ContextCard {
        key: Key,
        index: usize,
        limit: usize,
    },
    /// Chain cards of record context `context` (`@sat:app.cards.records`).
    RecordCards {
        context: usize,
    },
    /// Details of chain card `index` of record context `context`.
    RecordCard {
        context: usize,
        index: usize,
        limit: usize,
    },
    /// A window of the conflict table of the context that mod `key` belongs to.
    ContextTable {
        key: Key,
        #[serde(default)]
        order: ColumnOrder,
        /// The selected chain (index into the context's chains): `Activity` puts its stacks first.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        focus: Option<u32>,
        /// Only the rows of this chain's mods (the solution front).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        only: Option<u32>,
        row_start: usize,
        row_count: usize,
        col_start: usize,
        col_count: usize,
    },
    /// The order advisor for the file context of mod `key` (`@sat:app.advisor`).
    OrderAdvice {
        key: Key,
        #[serde(default)]
        strictness: sat_core::advisor::Strictness,
        /// Blend the current order into the score (the monolith's default).
        #[serde(default = "yes")]
        current_truth: bool,
        /// Mods that keep their places.
        #[serde(default)]
        locked: Vec<Key>,
    },
    /// What of the user's chain work no chain answers to (`@sat:app.orphans`).
    ChainOrphans,
    /// The file explorer (`@sat:app.tree`): a tree of mods, of the selection or of a chain.
    FileTree {
        subject: TreeSubject,
        #[serde(default)]
        mode: sat_core::filetree::Mode,
        /// A chain to light (signature).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        focus: Option<String>,
        /// Folders the user opened / closed (paths).
        #[serde(default)]
        opened: Vec<String>,
        #[serde(default)]
        closed: Vec<String>,
        #[serde(default = "tree_limit")]
        limit: usize,
    },
    /// Who has a path: the stack, disabled owners, its chain.
    PathOwners {
        path: String,
    },
    /// A file's copies and the preview of one (`@sat:app.preview`); `copy` — index into the copies, the game's
    /// when absent.
    FilePreview {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        copy: Option<u32>,
    },
    /// One folder of the whole build (`""` — the root).
    VfsDir {
        #[serde(default)]
        dir: String,
    },
    /// Paths containing a text.
    FindPaths {
        text: String,
        #[serde(default = "tree_limit")]
        limit: usize,
    },
    /// Pinned files.
    PinnedFiles,
    /// Folders outside the build the explorer shows (the game, the profile, the instance, the game's documents).
    Places,
    /// One folder of a place (`""` — its root); archives are folders.
    PlaceDir {
        place: String,
        #[serde(default)]
        dir: String,
    },
    /// The preview of a file of a place.
    PlaceFile {
        place: String,
        path: String,
    },
    /// A data path as the game loads it (the build's copy, else the game's): a mesh's texture.
    Asset {
        path: String,
    },
    /// The skeleton an animation (or a skinned mesh) plays on.
    SkeletonFor {
        path: String,
    },
    /// Skinned meshes to put on an animation's skeleton.
    BodyFor {
        path: String,
    },
    /// A small picture of an image file (a build path, or a file of `place`).
    Thumb {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        place: Option<String>,
        #[serde(default = "thumb_side")]
        side: u32,
    },
    /// The build indicators measured over the whole build (`@sat:app.indicators`).
    Indicators,
    /// The buffer's areas.
    Buffer,
    /// The selection summary of a view.
    Selection {
        view: ViewId,
    },
    /// What the decoration editor edits: the mods of the view's selection (the focused one when none) and
    /// their distinct styles with counts, so the editor can mark fields that differ ("mixed").
    Decoration {
        view: ViewId,
    },
    /// The flow layout window of a view (ADR 0012 §2): lines of cards that cross `[y, y + height)`.
    Lines {
        view: ViewId,
        y: u32,
        height: u32,
        flow: FlowParams,
        #[serde(default)]
        badges: u8,
        /// Bricks: how wide the window draws badges and counters (it measures its own font once).
        #[serde(default)]
        measures: Option<BrickMeasures>,
    },
}

/// Bricks' extras of a view: what they were measured with (badge density, the window's measures) and the width of
/// badges and counters per mod.
type BrickExtras = ((u8, Option<BrickMeasures>), std::collections::HashMap<Key, u32>);

/// What the window measured of its badges and counters, tenths of a pixel (`@sat:app.bricks`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct BrickMeasures {
    /// One glyph of the counters' font (monospace).
    pub digit10: u32,
    /// Between two counters.
    pub counter_gap10: u32,
    /// A context badge without its chains (dot, padding, border).
    pub context10: u32,
    /// One chain icon.
    pub chain10: u32,
    /// The «+N» of a context with more chains.
    pub more10: u32,
    /// The indicator and the gap after it (the left side starts with it; the right side mirrors the wider side).
    #[serde(default)]
    pub lead10: u32,
}

impl BrickMeasures {
    const FALLBACK: Self = Self {
        digit10: 60,
        counter_gap10: 60,
        context10: 140,
        chain10: 150,
        more10: 200,
        lead10: 150,
    };
}

// @sat:app.patch-style
/// Applies a JSON merge patch (RFC 7386) to a card style; `None` when nothing is left.
fn merge_style(cur: Option<&CardStyle>, patch: &serde_json::Value) -> Result<Option<CardStyle>, String> {
    fn merge(into: &mut serde_json::Value, patch: &serde_json::Value) {
        match (into, patch) {
            (serde_json::Value::Object(a), serde_json::Value::Object(p)) => {
                for (k, v) in p {
                    if v.is_null() {
                        a.remove(k);
                    } else if v.is_object() && a.get(k).is_none_or(serde_json::Value::is_object) {
                        // a part the entry does not have yet starts from its defaults
                        let part = a.entry(k.clone()).or_insert_with(|| part_default(k));
                        merge(part, v);
                    } else {
                        a.insert(k.clone(), v.clone());
                    }
                }
            }
            (into, p) => *into = p.clone(),
        }
    }
    fn part_default(k: &str) -> serde_json::Value {
        match k {
            "fill" => serde_json::json!({"kind": "solid", "color": 0x3b82f6}),
            "outline" => serde_json::json!({"radius": 8, "width": 1, "style": "solid", "color": 0x94a3b8}),
            "accent" => serde_json::json!({"color": 0xfacc15, "width": 4}),
            "watermark" => {
                serde_json::json!({"emoji": "✨", "position": "right", "size": 28, "opacity": 35, "blur": 0})
            }
            _ => serde_json::json!({}),
        }
    }
    let mut v = serde_json::to_value(cur.cloned().unwrap_or_default()).map_err(|e| e.to_string())?;
    merge(&mut v, patch);
    // a part left with no fields is no part (an editor that cleared the last text field cleared the text)
    if let serde_json::Value::Object(a) = &mut v {
        a.retain(|_, x| !x.as_object().is_some_and(serde_json::Map::is_empty));
    }
    let st: CardStyle = serde_json::from_value(v).map_err(|e| e.to_string())?;
    Ok((st != CardStyle::default()).then_some(st))
}

/// One distinct style among the edited entries (`None` = no style) and how many entries have it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StyleShare {
    pub style: Option<CardStyle>,
    pub count: usize,
}

/// One placed card of a flow line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CardView {
    /// Row of the card in the view (what `Navigate { row }` takes).
    pub index: u32,
    pub x: u32,
    pub width: u32,
    pub height: u32,
    /// The anchor's row for an element without an indicator (`att`), else the card's own row.
    pub row: RowView,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub att: Option<AttachView>,
}

/// One line of the flow layout: a separator header or a line of cards.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LineView {
    pub y: u32,
    pub height: u32,
    pub header: bool,
    pub cards: Vec<CardView>,
}

/// One separator tile: its block's size and how many mods in it are on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SeparatorTile {
    pub key: Key,
    pub name: String,
    pub index: u32,
    /// Position of the first mod of the block (MO2 priority), if the block has any.
    pub first: Option<u32>,
    pub mods: u32,
    pub enabled: u32,
    /// Nesting level (ADR 0016).
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub level: u8,
}

fn is_zero_u8(n: &u8) -> bool {
    *n == 0
}

// @sat:app.mod-detail
/// One opponent of a mod: who overrides whom and on how many files.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Opponent {
    /// Book key (absent for Overwrite, which is not an entry of the list).
    pub key: Option<Key>,
    pub name: String,
    pub pos: Option<u32>,
    /// Separator of the participant's block (participants are grouped by it, like the monolith).
    pub separator: Option<String>,
    /// This row is the focused mod itself.
    pub me: bool,
    pub enabled: bool,
    /// Files where this mod overrides the focused one.
    pub beats_me: u32,
    /// Files where the focused mod overrides this one.
    pub i_beat: u32,
    /// One tooth per listed chain of the focused mod (same order as `chains`): `w` this participant wins that
    /// chain, `l` it is overridden there, `-` it is not in that chain.
    pub teeth: String,
}

/// One exact chain of the focused mod.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChainView {
    /// Normalized chain (`@sat:routes`) — valid until the overwrite projection is recomputed.
    pub id: u32,
    pub signature: String,
    pub done: bool,
    pub emoji: String,
    pub hue: u16,
    /// What the chain is about (`@sat:naming.title`: strongest folders, separators, extensions), or the user's name.
    pub title: String,
    /// `title · lowest → winner` (monolith chain names), unique; or the user's name.
    pub name: String,
    /// The user named, iconed or coloured it.
    #[serde(default)]
    pub custom: bool,
    /// Losing copies against the winner's bytes (`@sat:identical.chain`); `None` — not compared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<sat_core::identical::ChainContent>,
    pub files: u32,
    pub bytes: u64,
    /// Stack, winner first.
    pub stack: Vec<String>,
    /// Place of the focused mod in the stack (0 = winner).
    pub place: usize,
    pub samples: Vec<String>,
}

/// The summary of one mod.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModDetail {
    pub key: Key,
    pub name: String,
    pub counters: Option<Counters>,
    pub opponents: Vec<Opponent>,
    pub chains: Vec<ChainView>,
    /// Chains not listed (the list keeps the largest).
    pub more_chains: usize,
    pub plugins: Vec<PluginView>,
    /// Tags, stickers and note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub marks: Option<sat_core::order::Marks>,
}

/// One row as a window shows it (ADR 0008 row + the mod card counters).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RowView {
    pub key: Key,
    pub pos: u32,
    pub kind: RowKind,
    pub name: String,
    pub enabled: bool,
    pub depth: u8,
    pub collapsed: bool,
    pub color: Option<u32>,
    pub size: u32,
    pub muted: bool,
    pub focused: bool,
    pub selected: bool,
    /// Overwrite counters of a mod: wins, losses, middle, unique; X = fully overridden.
    pub counters: Option<Counters>,
    /// The project's own counters (`@sat:counters`), rendered for this mod.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom: Vec<sat_core::counters::CounterValue>,
    /// Card style (ADR 0012).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<CardStyle>,
    /// Overwrite badges, one per context of the mod (empty when not asked or no conflicts).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub badges: Vec<ContextBadge>,
    /// Tags, stickers and note of the entry (`@sat:order.decor.marks`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub marks: Option<sat_core::order::Marks>,
    /// Stickers the project's rules and the project itself give the mod (`planned`, `new`) — beside the user's own
    /// in `marks` (the card draws both: labels above its top-left corner, a contour per sticker around it).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rule_stickers: Vec<String>,
    /// A member of an open logical group (the family outline) or a collapsed group's row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<GroupMark>,
}

/// The logical group a row belongs to, as the window outlines it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupMark {
    pub name: String,
    /// The first member in canon order (the outline carries the name there).
    pub first: bool,
    pub last: bool,
    pub size: u32,
}

// @sat:app.badges
/// One context badge of a mod card (monolith "classic" overwrite badges): the context's colour and the icons of
/// the mod's largest chains in it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextBadge {
    pub hue: u16,
    pub chains: Vec<ChainBadge>,
    /// Chains of this mod in the context not shown as icons.
    pub more: u32,
    /// Conflict files of this mod in the context.
    pub files: u32,
}

/// One chain icon.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChainBadge {
    /// Content signature, hex (`@sat:identity.of`).
    pub signature: String,
    /// Settled by the user: shown as a check mark.
    pub done: bool,
    pub emoji: String,
    pub hue: u16,
    pub files: u32,
    /// Place of the mod in the chain's stack (0 = it wins there).
    pub place: u8,
    /// Stack, winner first (tooltip).
    pub stack: Vec<String>,
}

/// The counter group at the bottom right of a mod card.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counters {
    pub win: u32,
    pub lose: u32,
    pub mid: u32,
    pub unique: u32,
    pub x: bool,
    pub plugins: u32,
}

/// One plugin as the plugin window shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginView {
    pub name: String,
    pub index: Option<String>,
    pub colour: sat_core::plugins::Colour,
    pub listed: sat_core::plugins::Listed,
    pub active: bool,
    pub owner: Option<String>,
    pub versions: usize,
    pub missing_masters: Vec<String>,
    /// What it does in the record layer (`@sat:records.plugin-stat`), once computed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub records: Option<sat_core::records::PluginRecordStat>,
}

/// Answer to a query.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Reply {
    /// `focus_row`: the row of the keyboard focus in this view (the window scrolls to keep it visible).
    Rows {
        view: ViewId,
        len: usize,
        focus_row: Option<usize>,
        rows: Vec<RowView>,
    },
    Totals {
        overwrite: OverwriteStats,
        plugins: PluginStats,
        revision: u64,
    },
    Plugins {
        plugins: Vec<PluginView>,
    },
    Keymap {
        bindings: Vec<(String, String)>,
    },
    ModDetail {
        detail: Box<ModDetail>,
    },
    RecordChains {
        page: Box<records::RecordPage>,
    },
    RecordContexts {
        contexts: Vec<records::RecordContextItem>,
    },
    ModSheet {
        sheet: Box<sheet::ModSheet>,
    },
    Found {
        found: find::Found,
    },
    GroupInside {
        inside: Option<GroupInside>,
    },
    Separators {
        tiles: Vec<SeparatorTile>,
    },
    Indicators {
        indicators: Vec<sat_core::counters::IndicatorValue>,
        /// What they are made of (the built-in ones when the project has none).
        defs: Vec<sat_core::counters::IndicatorDef>,
    },
    Buffer {
        areas: Vec<BufferAreaView>,
    },
    Selection {
        selection: SelectionView,
    },
    Decoration {
        keys: Vec<Key>,
        styles: Vec<StyleShare>,
    },
    Contexts {
        contexts: Vec<ContextItem>,
    },
    /// Tags in use (name, entries, colour) and the catalog of collections.
    Tags {
        tags: Vec<TagView>,
        catalog: sat_core::tags::TagCatalog,
    },
    Sync {
        records: Vec<RecordView>,
        plan: Box<sat_core::sync::plan::Plan>,
        /// What the commit writes besides the plan, when the shell gives the full plan (`@sat:app.sync.full`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        extras: Option<sync::PlanExtras>,
    },
    Pass {
        pass: pass::PassView,
    },
    Project {
        project: project::ProjectState,
    },
    Commit {
        commit: commit::CommitView,
    },
    FilePatch {
        file: patches::FilePatchView,
    },
    Domains {
        domains: Vec<DomainItem>,
    },
    StackTargets {
        targets: Vec<String>,
        total: usize,
    },
    /// `None` when there is no such domain.
    DomainContexts {
        contexts: Option<Vec<DomainContextItem>>,
    },
    /// `None` when there is no such context.
    ChainCards {
        cards: Option<Vec<ChainCard>>,
    },
    OrderAdvice {
        advice: Option<OrderAdvice>,
    },
    FileTree {
        tree: Option<FileTree>,
    },
    PathOwners {
        owners: Option<PathOwners>,
    },
    FilePreview {
        file: Option<Box<FilePreview>>,
    },
    VfsDir {
        entries: Vec<VfsEntry>,
    },
    FoundPaths {
        paths: Vec<String>,
        total: usize,
    },
    PinnedFiles {
        files: Vec<PinnedFile>,
    },
    Places {
        places: Vec<sat_formats::preview::Place>,
    },
    PlaceDir {
        entries: Vec<sat_formats::preview::DiskEntry>,
    },
    PlaceFile {
        file: Option<Box<PlaceFile>>,
    },
    Asset {
        asset: Option<Box<Asset>>,
    },
    Thumb {
        picture: Option<sat_formats::preview::Picture>,
    },
    Assets {
        assets: Vec<Asset>,
    },
    ChainOrphans {
        orphans: find::Orphans,
    },
    ChainCard {
        card: Option<Box<ChainCardDetail>>,
    },
    /// `None` when the mod is in no context.
    ContextTable {
        table: Option<Box<ContextTable>>,
    },
    /// `focus`: the line (y, height) of the keyboard focus, so the page keeps it in view.
    Lines {
        view: ViewId,
        total_height: u32,
        len: usize,
        focus: Option<(u32, u32)>,
        lines: Vec<LineView>,
    },
}

// @sat:app.event
/// What a command changed: per-view row patches (the UI re-reads its window) and whether the counters and
/// plugin list may have changed (canon / facts touched).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Event {
    Changed {
        revision: u64,
        views: Vec<ViewChange>,
        canon: bool,
    },
    ViewOpened {
        view: ViewId,
    },
    ViewClosed {
        view: ViewId,
    },
    /// A decision changed: badges and the summary re-read.
    Decided,
    /// The command was valid but changed nothing.
    Nothing,
}

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error(transparent)]
    Order(#[from] OrderError),
    #[error(transparent)]
    Command(#[from] CommandError),
    #[error("no key binding for {0}")]
    Unbound(String),
    #[error("bad chord {0:?}: {1}")]
    Chord(String, String),
    #[error("bad chain signature {0:?}")]
    Signature(String),
    #[error("file {0}")]
    File(String),
    #[error("sync record {0}")]
    Record(String),
    #[error("no plugin {0:?}")]
    Plugin(String),
    #[error(transparent)]
    PluginOrder(#[from] sat_core::plugin_edit::PluginOrderError),
    #[error("bad style patch: {0}")]
    Style(String),
    #[error("{0}")]
    Expr(String),
}

// @sat:app.dispatch
/// One open build: the session and the resources active on this machine.
#[derive(Debug)]
pub struct App {
    session: Session,
    registry: Registry,
    revision: u64,
    /// Flow index per view, valid for one book revision and one set of parameters.
    flows: std::collections::HashMap<ViewId, FlowIndex>,
    /// Bricks: what each container holds besides its name, by view (`@sat:app.bricks`), built with its flow index.
    /// Per view: what the extras were measured with (badge density, the window's measures) and the extras.
    brick_extra: std::collections::HashMap<ViewId, BrickExtras>,
    /// Sorted columns of the context table that is open.
    open_context: Option<context::OpenContext>,
    /// Sync records and the folder names they were found against.
    sync: sync::SyncState,
    /// Tag collections and colours (`@sat:tags`).
    tags: sat_core::tags::TagCatalog,
    /// Headings of the normalized chains, per overwrite computation.
    heads: Option<chains::Cached>,
    /// Content hashes of the table's providers (`@sat:identical`) and what each mod loses as equal copies, per
    /// (overwrite computation, hashes revision).
    /// The record layer as the shell computed it (`@sat:app.records`).
    records: Arc<records::RecordReport>,
    record_state: records::RecordState,
    content_hashes: Arc<sat_core::identical::ContentHashes>,
    content_rev: u64,
    mod_content: Option<chains::CachedContent>,
    /// Conditional stickers of the project (`@sat:sticker-rules`).
    sticker_rules: sat_core::sticker_rules::StickerRules,
    /// Chain identities the user set (`@sat:naming.manual`), and their revision (heads are rebuilt on a change).
    chain_identities: Vec<sat_core::naming::ManualIdentity>,
    chain_identities_rev: u64,
    /// The user's merges and splits of chains (`@sat:layout`).
    chain_layout: sat_core::layout::ChainLayout,
    chain_layout_rev: u64,
    /// Members of chains the user touched (`@sat:lineage`).
    chain_lineage: sat_core::lineage::ChainLineage,
    chain_lineage_rev: u64,
    /// Reads a copy of a file for the explorer (`@sat:app.preview`), set by the shell.
    preview_source: preview::SourceSlot,
    /// Files the user pinned (the project keeps them).
    pinned_files: Vec<String>,
    /// The build's folders, built once per table.
    vfs: Option<Arc<preview::VfsIndex>>,
    /// The project's counters (`@sat:counters`).
    counters: sat_core::counters::Counters,
    /// Build indicators (`@sat:counters.build`) and the project's definitions of them (empty: the built-in ones).
    indicators: sat_core::counters::Indicators,
    indicator_defs: Vec<sat_core::counters::IndicatorDef>,
    /// Where mods come from, by folded name (`@sat:source`).
    sources: std::collections::HashMap<String, sat_core::source::ModSource>,
    /// The project's own stickers by folded mod name (`planned`, `new`).
    system_stickers: std::collections::HashMap<String, Vec<String>>,
    /// Conflict domains beyond files and records, and what the shell hands over for them (`@sat:app.domains`).
    domain_cache: domains::DomainCache,
    domain_inputs: domains::DomainInputs,
    /// The run as the window sees it (`@sat:app.pass`).
    pass: pass::PassSlot,
    /// The project as loaded (`@sat:app.project`): what the window does not edit, carried to saves and plans.
    project_base: Option<sat_core::project::Project>,
    /// The full commit plan of the build, from the shell (`@sat:app.sync.full`).
    plan_source: Option<Arc<dyn sync::PlanSource>>,
    /// Saving the project (`@sat:app.project.state`).
    save: project::SaveSlot,
    /// The commit (`@sat:app.commit`).
    commit: commit::CommitSlot,
    /// File patches as edited, sources by mod name (`@sat:app.patches`).
    patches: Vec<sat_core::patch::FilePatch>,
}

impl App {
    pub fn new(session: Session, registry: Registry) -> Self {
        Self {
            session,
            registry,
            revision: 0,
            flows: Default::default(),
            brick_extra: Default::default(),
            open_context: None,
            heads: None,
            sync: Default::default(),
            tags: Default::default(),
            sticker_rules: Default::default(),
            chain_identities: Vec::new(),
            chain_identities_rev: 0,
            chain_layout: Default::default(),
            chain_layout_rev: 0,
            chain_lineage: Default::default(),
            chain_lineage_rev: 0,
            preview_source: Default::default(),
            pinned_files: Vec::new(),
            project_base: None,
            patches: Vec::new(),
            commit: Default::default(),
            save: Default::default(),
            plan_source: None,
            pass: Default::default(),
            vfs: None,
            records: Arc::default(),
            record_state: records::RecordState::Off,
            content_hashes: Arc::default(),
            content_rev: 0,
            mod_content: None,
            system_stickers: Default::default(),
            counters: Default::default(),
            indicators: sat_core::counters::Indicators::compile(&sat_core::counters::default_indicators()).0,
            indicator_defs: Vec::new(),
            sources: Default::default(),
            domain_cache: Default::default(),
            domain_inputs: Default::default(),
        }
    }

    /// The shell hands over the project's tag catalog.
    pub fn set_tag_catalog(&mut self, catalog: sat_core::tags::TagCatalog) {
        self.tags = catalog;
    }

    pub fn session(&mut self) -> &mut Session {
        &mut self.session
    }

    /// Runs a command; returns what changed.
    pub fn command(&mut self, c: Command) -> Result<Event, AppError> {
        use Command as C;
        let changed: Option<order::Change> = match c {
            C::Key { view, chord } => {
                let ch = Chord::parse(&chord).map_err(|e| AppError::Chord(chord.clone(), e))?;
                let id = self
                    .registry
                    .keymap()
                    .lookup("list", &ch)
                    .map(str::to_string)
                    .ok_or_else(|| AppError::Unbound(ch.to_string()))?;
                self.session.run(&self.registry, &id, view)?
            }
            C::Run { view, id } => self.session.run(&self.registry, &id, view)?,
            C::Navigate { view, nav, select } => self.session.command(|b| b.navigate(view, nav, select))?,
            C::ShiftSelection { view, how } => self.session.command(|b| b.shift_selection(view, how))?,
            C::Move { keys, before, after } => {
                let before = match after {
                    // the first entry after `after` that is not moving; none — the end
                    Some(a) => {
                        let b = self.session.book();
                        (b.pos(a) + 1..b.len())
                            .map(|p| b.key_at(p))
                            .find(|k| !keys.contains(k))
                    }
                    None => before,
                };
                self.session.command(|b| b.move_keys(&keys, before))?
            }
            C::Reorder { keys } => {
                let items = self.reorder_items(&keys);
                self.session.command(|b| b.place_keys(&items))?
            }
            C::SetEnabled { keys, enabled } => self.session.command(|b| b.set_enabled(&keys, enabled))?,
            C::SetColor { keys, color } => self.session.command(|b| b.set_color(&keys, color))?,
            C::SetLevel { keys, level } => self.session.command(|b| b.set_level(&keys, level))?,
            C::SetStyle { keys, style } => self.session.command(|b| b.set_style(&keys, style))?,
            // @sat:app.marks
            C::SetIndicators { defs } => {
                let (_, errors) = sat_core::counters::Indicators::compile(&defs);
                if let Some(e) = errors.into_iter().next() {
                    return Err(AppError::Expr(e));
                }
                self.set_indicators(&defs);
                self.revision += 1;
                return Ok(Event::Decided);
            }
            // @sat:app.buffer
            C::Buffer { edit } => {
                let mut areas = self.session.book().buffer().to_vec();
                match edit {
                    BufferEdit::Put { keys, area } => {
                        for a in &mut areas {
                            a.1.retain(|k| !keys.contains(k));
                        }
                        match area.filter(|&i| i < areas.len()) {
                            Some(i) => areas[i].1.extend(keys),
                            None => areas.push((String::new(), keys)),
                        }
                        self.session.command(|b| b.set_buffer(areas))?
                    }
                    BufferEdit::Remove { key } => {
                        for a in &mut areas {
                            a.1.retain(|&k| k != key);
                        }
                        self.session.command(|b| b.set_buffer(areas))?
                    }
                    BufferEdit::Clear { area } => {
                        if area < areas.len() {
                            areas.remove(area);
                        }
                        self.session.command(|b| b.set_buffer(areas))?
                    }
                    BufferEdit::Rename { area, name } => {
                        if let Some(a) = areas.get_mut(area) {
                            a.0 = name;
                        }
                        self.session.command(|b| b.set_buffer(areas))?
                    }
                    BufferEdit::Place { area, before } => {
                        let Some((_, keys)) = areas.get(area).cloned() else {
                            return Ok(Event::Nothing);
                        };
                        // the anchor itself in the area: place before the next entry not in it
                        let moved = self
                            .session
                            .command(|b| b.move_keys(&keys, before.filter(|k| !keys.contains(k))))?;
                        areas.remove(area);
                        self.session.command(|b| b.set_buffer(areas))?;
                        moved
                    }
                }
            }
            // @sat:app.groups
            C::CreateGroup { name, keys } => self
                .session
                .command(|b| b.create_group(&name, &keys).map(|(_, c)| Some(c)))?,
            C::DissolveGroup { key } => self.session.command(|b| match b.group_id_of(key) {
                Some(g) => b.dissolve_group(g),
                None => Ok(None),
            })?,
            C::SetGroupCollapsed { view, key, collapsed } => {
                self.session.command(|b| match b.group_id_of(key) {
                    Some(g) => b.set_group_collapsed(view, g, collapsed),
                    None => Ok(None),
                })?
            }
            // @sat:app.attached
            C::Attach { key, edit } => self.session.command(|b| {
                let mut items = b.attached(key).to_vec();
                match edit {
                    AttachEdit::Add { kind, width, height } => items.push(sat_core::order::Attachment {
                        id: b.last_attachment_id() + 1,
                        kind,
                        width,
                        height,
                        break_before: false,
                        full_width: false,
                    }),
                    AttachEdit::Update { attachment } => {
                        if let Some(a) = items.iter_mut().find(|a| a.id == attachment.id) {
                            *a = attachment;
                        }
                    }
                    AttachEdit::Remove { id } => items.retain(|a| a.id != id),
                }
                b.set_attached(key, items)
            })?,
            C::EditMarks { keys, edit } => self.session.command(|b| {
                b.patch_marks(&keys, |mut m| {
                    match &edit {
                        MarksEdit::AddTag { tag } => m.tags.push(tag.clone()),
                        MarksEdit::RemoveTag { tag } => {
                            // case-folded like the normal form (Cyrillic too)
                            let gone = tag.trim().to_lowercase();
                            m.tags.retain(|t| t.to_lowercase() != gone)
                        }
                        MarksEdit::AddSticker { sticker } => m.stickers.push(sticker.clone()),
                        MarksEdit::RemoveSticker { sticker } => m.stickers.retain(|s| s != sticker),
                        MarksEdit::SetNote { note } => m.note = note.clone(),
                    }
                    m
                })
            })?,
            C::PatchStyle { keys, patch } => {
                let mut bad = None;
                let r = self.session.command(|b| {
                    b.patch_style(&keys, |cur| merge_style(cur, &patch))
                        .or_else(|e| match e {
                            order::PatchStyleError::Order(e) => Err(e),
                            order::PatchStyleError::Patch(e) => {
                                bad = Some(e);
                                Ok(None)
                            }
                        })
                });
                if let Some(e) = bad {
                    return Err(AppError::Style(e));
                }
                r?
            }
            C::SetCollapsed {
                view,
                separator,
                collapsed,
            } => self
                .session
                .command(|b| b.set_collapsed(view, separator, collapsed))?,
            C::SetFilter { view, filter } => self.session.command(|b| b.set_filter(view, filter))?,
            C::Focus { view, key } => self.session.command(|b| Ok(b.focus_entry(view, key)))?,
            // @sat:app.plugins-edit
            C::SetPluginChecked { name, checked } => {
                let list = self.session.plugins();
                let p = list.get(&name).ok_or_else(|| AppError::Plugin(name.clone()))?;
                // a plugin of disabled mods: checking it enables the mod that would provide it
                let owner = (checked && !p.present)
                    .then(|| {
                        p.winner
                            .map(|w| self.session.table().mod_name(p.versions[w].owner).to_string())
                    })
                    .flatten();
                let mut canon = self.session.plugin_canon().clone();
                let mut changed = canon.set_checked(&name, checked);
                let mut views = Vec::new();
                if let Some(owner) = owner {
                    let key = self.session.book().key_of(&owner);
                    if let Some(k) = key
                        && let Some(ch) = self.session.command(|b| b.set_enabled(&[k], Some(true)))?
                    {
                        views = ch.views;
                        changed = true;
                    }
                }
                if !changed {
                    return Ok(Event::Nothing);
                }
                self.session.set_plugin_canon(canon);
                self.revision += 1;
                return Ok(Event::Changed {
                    revision: self.revision,
                    views,
                    canon: true,
                });
            }
            C::SetConflictIgnore { rules } => {
                let rules = sat_core::ignore::IgnoreRules::new(rules);
                if &rules == self.session.conflict_ignore() {
                    return Ok(Event::Nothing);
                }
                self.session.set_conflict_ignore(rules);
                self.revision += 1;
                return Ok(Event::Changed {
                    revision: self.revision,
                    views: Vec::new(),
                    canon: true,
                });
            }
            C::SetPluginHidden { name, hidden } => {
                let mut canon = self.session.plugin_canon().clone();
                if !canon.set_blacklisted(&name, hidden) {
                    return Ok(Event::Nothing);
                }
                self.session.set_plugin_canon(canon);
                self.revision += 1;
                return Ok(Event::Changed {
                    revision: self.revision,
                    views: Vec::new(),
                    canon: true,
                });
            }
            // @sat:app.chain-identity
            C::SetChainIdentity {
                signature,
                name,
                emoji,
                hue,
            } => {
                let heads = self.heads();
                let h = heads
                    .iter()
                    .find(|h| h.hex() == signature)
                    .ok_or_else(|| AppError::Signature(signature.clone()))?;
                let touched = h.chains.clone();
                let id = sat_core::naming::ManualIdentity {
                    sources: h.keys.clone(),
                    name: name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()),
                    emoji,
                    hue,
                };
                // the identity this chain wears now (inherited or its own) is replaced
                let srcs = [h.keys.as_slice()];
                let worn = sat_core::naming::inherit(&srcs, &self.chain_identities)[0];
                let mut list = self.chain_identities.clone();
                if let Some(i) = worn {
                    list.remove(i);
                }
                if !id.is_empty() {
                    list.push(id);
                }
                if list == self.chain_identities {
                    return Ok(Event::Nothing);
                }
                self.touch(&touched);
                self.chain_identities = list;
                self.chain_identities_rev += 1;
                self.revision += 1;
                return Ok(Event::Changed {
                    revision: self.revision,
                    views: Vec::new(),
                    canon: true,
                });
            }
            // @sat:app.layout
            C::MergeChains { signatures } => {
                let mut chains: Vec<Vec<sat_core::ids::ChainId>> = Vec::new();
                {
                    let heads = self.heads();
                    for sig in &signatures {
                        let h = heads
                            .iter()
                            .find(|h| &h.hex() == sig)
                            .ok_or_else(|| AppError::Signature(sig.clone()))?;
                        chains.push(h.chains.clone());
                    }
                }
                let touched = self.touch(&chains.concat());
                // after touching, every chain is known by its own signature
                let ow = self.session.overwrite();
                let sets: Vec<Vec<u64>> = chains
                    .iter()
                    .map(|cs| {
                        cs.iter()
                            .map(|&c| chains::exact_signature(&ow, &self.session, c))
                            .collect()
                    })
                    .collect();
                let refs: Vec<&[u64]> = sets.iter().map(Vec::as_slice).collect();
                let mut l = self.chain_layout.clone();
                if sets.len() < 2 || !l.merge(&refs) {
                    if touched {
                        self.revision += 1;
                        return Ok(Event::Decided);
                    }
                    return Ok(Event::Nothing);
                }
                return Ok(self.set_layout(l));
            }
            C::PinFile { path, pinned } => {
                return Ok(if self.pin_file(&path, pinned) {
                    self.revision += 1;
                    Event::Decided
                } else {
                    Event::Nothing
                });
            }
            C::PruneOrphans => {
                return Ok(if self.prune_orphans() > 0 {
                    self.revision += 1;
                    Event::Decided
                } else {
                    Event::Nothing
                });
            }
            C::SplitChain { signature } => {
                let chains = self.head_chains(&signature)?;
                self.touch(&chains);
                let ow = self.session.overwrite();
                let srcs: Vec<u64> = chains
                    .iter()
                    .map(|&c| chains::exact_signature(&ow, &self.session, c))
                    .collect();
                let mut l = self.chain_layout.clone();
                if srcs.len() < 2 || !l.split(&srcs) {
                    return Ok(Event::Nothing);
                }
                return Ok(self.set_layout(l));
            }
            C::ResetChain { signature } => {
                let srcs = self.head_sources(&signature)?;
                let mut l = self.chain_layout.clone();
                if !l.reset(&srcs) {
                    return Ok(Event::Nothing);
                }
                return Ok(self.set_layout(l));
            }
            C::MovePlugins { names, before } => {
                let list = self.session.plugins();
                let mut canon = self.session.plugin_canon().clone();
                if !canon.move_plugins(&list, &names, before.as_deref())? {
                    return Ok(Event::Nothing);
                }
                self.session.set_plugin_canon(canon);
                self.revision += 1;
                return Ok(Event::Changed {
                    revision: self.revision,
                    views: Vec::new(),
                    canon: true,
                });
            }
            C::CascadePlugins => {
                let list = self.session.plugins();
                let canon = sat_core::plugin_edit::cascade(&list, self.session.plugin_canon());
                if &canon == self.session.plugin_canon() {
                    return Ok(Event::Nothing);
                }
                self.session.set_plugin_canon(canon);
                self.revision += 1;
                return Ok(Event::Changed {
                    revision: self.revision,
                    views: Vec::new(),
                    canon: true,
                });
            }
            C::StartPass { mode } => {
                return Ok(if self.request_pass(mode) {
                    Event::Decided
                } else {
                    Event::Nothing
                });
            }
            C::PatchFile { mod_name, path, edit } => {
                return Ok(if self.patch_file(&mod_name, &path, &edit)? {
                    self.revision += 1;
                    Event::Decided
                } else {
                    Event::Nothing
                });
            }
            C::Commit { confirmed } => {
                return Ok(if self.request_commit(confirmed) {
                    Event::Decided
                } else {
                    Event::Nothing
                });
            }
            C::SaveProject { force } => {
                return Ok(if self.request_save(force) {
                    Event::Decided
                } else {
                    Event::Nothing
                });
            }
            C::CancelPass => {
                return Ok(if self.request_cancel() {
                    Event::Decided
                } else {
                    Event::Nothing
                });
            }
            C::Decide {
                fingerprint,
                decision,
            } => {
                return Ok(if self.decide(&fingerprint, decision)? {
                    self.revision += 1;
                    Event::Decided
                } else {
                    Event::Nothing
                });
            }
            C::SetChainDone { signature, done } => {
                let sig = u64::from_str_radix(&signature, 16)
                    .map_err(|_| AppError::Signature(signature.clone()))?;
                // a normalized chain settles (or reopens) every exact chain in it
                let heads = self.heads();
                let mut changed = false;
                match heads.iter().find(|h| h.signature == sig) {
                    // settling stamps every exact chain with what it is now (`@sat:decisions.stamp`) and
                    // remembers its members (`@sat:lineage`); settling again renews the stamps ("confirm")
                    Some(h) if done => {
                        let ow = self.session.overwrite();
                        let chains = h.chains.clone();
                        changed |= self.touch(&chains);
                        for c in chains {
                            let st = chains::exact_stamp(&ow, &self.session, c);
                            let es = chains::exact_signature(&ow, &self.session, c);
                            changed |= self.session.set_chain_done_stamped(es, st);
                        }
                    }
                    Some(h) => {
                        for &s in &h.sources {
                            changed |= self.session.set_chain_done(s, false);
                        }
                    }
                    None => changed |= self.session.set_chain_done(sig, done),
                }
                return Ok(if changed {
                    self.revision += 1;
                    Event::Decided
                } else {
                    Event::Nothing
                });
            }
            C::Undo => self.session.command(|b| Ok(b.undo()))?,
            C::Redo => self.session.command(|b| Ok(b.redo()))?,
            C::OpenView => {
                let mut opened = None;
                self.session.command(|b| {
                    opened = Some(b.open_view()?);
                    Ok(None::<order::Change>)
                })?;
                return Ok(Event::ViewOpened {
                    view: opened.expect("opened"),
                });
            }
            C::CloseView { view } => {
                self.session
                    .command(|b| b.close_view(view).map(|()| None::<order::Change>))?;
                return Ok(Event::ViewClosed { view });
            }
        };
        Ok(match changed {
            None => Event::Nothing,
            Some(ch) => {
                self.revision += 1;
                let canon = ch.truth.iter().any(|t| {
                    matches!(
                        t.truth,
                        sat_core::engine::Truth::Canon | sat_core::engine::Truth::Facts
                    )
                });
                Event::Changed {
                    revision: self.revision,
                    views: ch.views,
                    canon,
                }
            }
        })
    }

    /// Row views `[start, start + count)` of a view: what a card shows, counters and badges included.
    fn row_views(&mut self, view: ViewId, start: usize, count: usize, badges: u8) -> Vec<RowView> {
        let heads = if badges > 0 { self.heads() } else { Arc::default() };
        let ow = self.session.overwrite();
        let plugins = self.session.plugins();
        let names = self.session.table().clone();
        let index: std::collections::HashMap<String, ModId> = names
            .mod_names()
            .iter()
            .enumerate()
            .map(|(i, n)| (n.to_lowercase(), ModId::from_idx(i)))
            .collect();
        let mut per_mod = vec![0u32; names.mod_count()];
        for p in &plugins.plugins {
            if let Some(w) = p.winner {
                per_mod[p.versions[w].owner.idx()] += 1;
            }
        }
        let book = self.session.book();
        let mut rows = book
            .rows(view, start, count)
            .into_iter()
            .map(|r| {
                let counters = (r.kind == RowKind::Mod)
                    .then(|| index.get(&r.name.to_lowercase()))
                    .flatten()
                    .map(|&m| {
                        let s = ow.stat(m);
                        Counters {
                            win: s.win,
                            lose: s.lose,
                            mid: s.mid,
                            unique: s.unique,
                            x: s.fully_overridden,
                            plugins: per_mod[m.idx()],
                        }
                    });
                RowView {
                    key: r.key,
                    pos: r.pos,
                    kind: r.kind,
                    name: r.name.to_string(),
                    enabled: r.enabled,
                    depth: r.depth,
                    collapsed: r.collapsed,
                    color: r.color,
                    size: r.size,
                    muted: r.muted,
                    focused: r.focused,
                    selected: r.selected,
                    style: r.style.cloned(),
                    marks: r.marks.cloned(),
                    badges: match (badges, index.get(&r.name.to_lowercase())) {
                        (n, Some(&m)) if n > 0 && r.kind == RowKind::Mod => {
                            mod_badges(&ow, &heads, &self.session, m, n)
                        }
                        _ => Vec::new(),
                    },
                    counters,
                    custom: Vec::new(),
                    rule_stickers: Vec::new(),
                    group: None,
                }
            })
            .collect::<Vec<_>>();
        let book = self.session.book();
        for r in &mut rows {
            if let Some((name, members)) = book.group_of(r.key) {
                r.group = Some(GroupMark {
                    name,
                    first: members.first() == Some(&r.key),
                    last: members.last() == Some(&r.key),
                    size: members.len() as u32,
                });
            }
        }
        for r in &mut rows {
            if r.kind == RowKind::Mod {
                r.rule_stickers = self.rule_stickers(r.key);
            }
        }
        rows
    }

    // @sat:app.indicators
    /// The project's build indicators (the built-in ones when it has none); returns those that did not compile.
    pub fn set_indicators(&mut self, defs: &[sat_core::counters::IndicatorDef]) -> Vec<String> {
        self.indicator_defs = defs.to_vec();
        let defs = if defs.is_empty() {
            sat_core::counters::default_indicators()
        } else {
            defs.to_vec()
        };
        let (c, errors) = sat_core::counters::Indicators::compile(&defs);
        self.indicators = c;
        errors
    }

    /// Every indicator over every mod of the build, in one pass over the mods' views.
    fn indicator_values(&mut self) -> Vec<sat_core::counters::IndicatorValue> {
        let keys: Vec<Key> = {
            let b = self.session.book();
            b.entries()
                .filter(|e| e.1 == sat_core::canon::EntryKind::Mod)
                .filter_map(|e| b.key_of(e.0))
                .collect()
        };
        let parts: Vec<_> = keys.iter().map(|&k| self.view_parts(k)).collect();
        self.indicators.measure(parts.iter().map(|p| p.view()))
    }

    // @sat:app.selection
    /// The selection summary (ADR 0020 §7 «Выделение»): mods, names, tags and stickers with counts, counter sums.
    fn selection_view(&mut self, view: ViewId) -> SelectionView {
        let keys: Vec<Key> = {
            let book = self.session.book();
            let mut ks: Vec<Key> = book
                .selection(view)
                .into_iter()
                .filter(|&k| book.kind(k) == sat_core::canon::EntryKind::Mod)
                .collect();
            ks.sort_by_key(|&k| book.pos(k));
            ks
        };
        let ow = self.session.overwrite();
        let names = self.session.table().clone();
        let index: std::collections::HashMap<String, ModId> = names
            .mod_names()
            .iter()
            .enumerate()
            .map(|(i, n)| (n.to_lowercase(), ModId::from_idx(i)))
            .collect();
        let plugins = self.session.plugins();
        let mut per_mod = vec![0u32; names.mod_count()];
        for p in &plugins.plugins {
            if let Some(w) = p.winner {
                per_mod[p.versions[w].owner.idx()] += 1;
            }
        }
        let book = self.session.book();
        let mut v = SelectionView {
            count: keys.len() as u32,
            ..Default::default()
        };
        let mut tags: std::collections::BTreeMap<String, (String, u32)> = Default::default();
        let mut stickers: std::collections::BTreeMap<String, u32> = Default::default();
        for &k in &keys {
            let name = book.name(k);
            v.enabled += u32::from(book.enabled(k));
            if v.names.len() < 40 {
                v.names.push((k, name.to_string()));
            }
            if let Some(m) = book.marks(k) {
                for t in &m.tags {
                    tags.entry(t.to_lowercase()).or_insert_with(|| (t.clone(), 0)).1 += 1;
                }
                for st in &m.stickers {
                    *stickers.entry(st.clone()).or_default() += 1;
                }
            }
            if let Some(&m) = index.get(&name.to_lowercase()) {
                let s = ow.stat(m);
                let c = &mut v.sums;
                c.win += s.win;
                c.lose += s.lose;
                c.mid += s.mid;
                c.unique += s.unique;
                c.plugins += per_mod[m.idx()];
                c.x |= s.fully_overridden;
            }
        }
        v.tags = tags.into_values().collect();
        v.stickers = stickers.into_iter().collect();
        v
    }

    // @sat:app.bricks
    /// What each mod container holds besides its name, in pixels, for the bricks layout (ADR 0020 §6): its badges
    /// and counters as the window draws them (one line, 10 px digits). So a container is as wide as its content and
    /// nothing covers the name unless the line is short of room.
    fn brick_extras(
        &mut self,
        view: ViewId,
        badges: u8,
        m: BrickMeasures,
    ) -> std::collections::HashMap<Key, u32> {
        let len = self.session.book().row_count(view);
        let digits = |n: u32| n.to_string().len() as u32;
        // counters as the window writes them: `W12`, `L3`, `M1`, `+40`, `2 esp`, `X`
        self.row_views(view, 0, len, badges)
            .into_iter()
            .filter(|r| matches!(r.kind, RowKind::Mod | RowKind::Group))
            .map(|r| {
                // a collapsed group: its unfold mark on the left, its size on the right
                if r.kind == RowKind::Group {
                    let right = digits(r.size) * m.digit10;
                    return (r.key, (2 * m.lead10.max(right)).div_ceil(10));
                }
                // the name stands in the middle of the container (monolith): the badges on the left and the
                // counters on the right each get the wider side's room, so at its natural width nothing covers it
                let (mut left, mut w) = (m.lead10, 0);
                if let Some(c) = &r.counters {
                    let mut shown = 0;
                    for (n, extra) in [(c.win, 1), (c.lose, 1), (c.mid, 1), (c.unique, 1), (c.plugins, 4)] {
                        if n > 0 {
                            w += (digits(n) + extra) * m.digit10;
                            shown += 1;
                        }
                    }
                    if c.x {
                        w += m.digit10;
                        shown += 1;
                    }
                    w += shown * m.counter_gap10;
                }
                for b in &r.badges {
                    left += m.context10
                        + b.chains.len() as u32 * m.chain10
                        + if b.more > 0 { m.more10 } else { 0 };
                }
                (r.key, (2 * left.max(w)).div_ceil(10))
            })
            .collect()
    }

    /// Answers a query (projections are brought up to date lazily, only what the query needs).
    pub fn query(&mut self, q: Query) -> Result<Reply, AppError> {
        Ok(match q {
            Query::Rows {
                view,
                start,
                count,
                badges,
            } => {
                let mut rows = self.row_views(view, start, count, badges);
                // @sat:app.counters
                if !self.counters.is_empty() {
                    for r in rows.iter_mut().filter(|r| r.kind == RowKind::Mod) {
                        let parts = self.view_parts(r.key);
                        r.custom = self.counters.render(&parts.view());
                    }
                }
                let book = self.session.book();
                let len = book.row_count(view);
                Reply::Rows {
                    view,
                    len,
                    focus_row: book.focus_row(view),
                    rows,
                }
            }
            Query::Totals => {
                let ow = self.session.overwrite();
                let pl = self.session.plugins();
                Reply::Totals {
                    overwrite: ow.totals(),
                    plugins: pl.stats,
                    revision: self.revision,
                }
            }
            Query::Plugins => {
                let pl: Arc<_> = self.session.plugins();
                let table = self.session.table().clone();
                let plugins = pl
                    .plugins
                    .iter()
                    .map(|p| PluginView {
                        name: p.name.clone(),
                        index: p.index.map(|i| i.to_string()),
                        colour: p.colour,
                        listed: p.listed,
                        active: p.active,
                        owner: p.winner.map(|w| table.mod_name(p.versions[w].owner).to_string()),
                        versions: p.versions.len(),
                        missing_masters: p.missing_masters.clone(),
                        records: self.plugin_records(&p.name),
                    })
                    .collect();
                Reply::Plugins { plugins }
            }
            Query::RecordChains {
                plugin,
                conflicts_only,
                offset,
                limit,
            } => Reply::RecordChains {
                page: Box::new(self.record_page(plugin.as_deref(), conflicts_only, offset, limit.min(1000))),
            },
            Query::ModDetail { key } => Reply::ModDetail {
                detail: Box::new(self.mod_detail(key)),
            },
            // @sat:app.group-inside
            Query::GroupInside { key } => Reply::GroupInside {
                inside: self.session.group_inside(key).map(|(name, members, ow)| {
                    let table = self.session.table();
                    let mut chains: Vec<InsideChain> = ow
                        .chain_ids()
                        .into_iter()
                        .map(|c| InsideChain {
                            stack: ow
                                .chain_mods(c)
                                .iter()
                                .map(|&m| table.mod_name(m).to_string())
                                .collect(),
                            files: ow.chain(c).files,
                        })
                        .collect();
                    chains.sort_by_key(|c| std::cmp::Reverse(c.files));
                    chains.truncate(CHAIN_LIMIT * 4);
                    GroupInside {
                        name,
                        members: members.iter().map(|&m| table.mod_name(m).to_string()).collect(),
                        conflict_paths: ow.totals().conflict_paths as u32,
                        chains,
                    }
                }),
            },
            Query::Find { expr, limit } => Reply::Found {
                found: self.find(&expr, limit)?,
            },
            Query::ModSheet { key, level } => Reply::ModSheet {
                sheet: Box::new(self.mod_sheet(key, level)),
            },
            // @sat:app.lines
            Query::Lines {
                view,
                y,
                height,
                flow,
                badges,
                measures,
            } => {
                let book = self.session.book();
                let rev = book.revision();
                let fresh = self
                    .flows
                    .get(&view)
                    .is_some_and(|i| i.revision == rev && i.params == flow)
                    // natural widths depend on the badges shown and on how the window measures them
                    && (flow.natural.is_none() || self.brick_extra.get(&view).is_some_and(|(k, _)| *k == (badges, measures)));
                if !fresh {
                    let extra = if flow.natural.is_some() {
                        self.brick_extras(view, badges, measures.unwrap_or(BrickMeasures::FALLBACK))
                    } else {
                        Default::default()
                    };
                    let book = self.session.book();
                    let idx =
                        book.flow_index_by(view, flow.clone(), &|k| extra.get(&k).copied().unwrap_or(0));
                    self.flows.insert(view, idx);
                    self.brick_extra.insert(view, ((badges, measures), extra));
                }
                let book = self.session.book();
                let extra = &self.brick_extra[&view].1;
                let by = |k: Key| extra.get(&k).copied().unwrap_or(0);
                let idx = &self.flows[&view];
                let lines = book.flow_lines_by(view, idx, y, height, &by);
                let focus = book
                    .focus_row(view)
                    .and_then(|r| book.flow_row_y_by(view, idx, r as u32, &by));
                let total_height = idx.total_height;
                let len = book.row_count(view);
                let (lo, hi) = lines
                    .iter()
                    .flat_map(|l| l.cards.iter().map(|c| c.row))
                    .fold((u32::MAX, 0), |(lo, hi), r| (lo.min(r), hi.max(r)));
                let mut rows: Vec<Option<RowView>> = if lo <= hi {
                    self.row_views(view, lo as usize, (hi - lo + 1) as usize, badges)
                        .into_iter()
                        .map(Some)
                        .collect()
                } else {
                    Vec::new()
                };
                // every row of the window is on exactly one line; an element without an indicator comes after its
                // anchor on the lines and carries a copy of the anchor's row
                let book = self.session.book();
                let mut anchors: std::collections::HashMap<u32, RowView> = Default::default();
                let mut take = |r: u32, att: bool| {
                    if att {
                        // the anchor's line may be above the window: its row is then still unused
                        return anchors
                            .get(&r)
                            .cloned()
                            .or_else(|| rows[(r - lo) as usize].clone())
                            .expect("the anchor's row is in the window");
                    }
                    let row = rows[(r - lo) as usize].take().expect("row placed once");
                    if !book.attached(row.key).is_empty() {
                        anchors.insert(r, row.clone());
                    }
                    row
                };
                let attach = |k: Key, id: u32| {
                    let a = book.attached(k).iter().find(|a| a.id == id)?.clone();
                    let summary = match &a.kind {
                        sat_core::order::AttachKind::Summary { tag } => {
                            let (total, enabled) = book.tag_members(tag);
                            Some(FamilyCount { total, enabled })
                        }
                        _ => None,
                    };
                    Some(AttachView {
                        attachment: a,
                        summary,
                    })
                };
                let lines = lines
                    .into_iter()
                    .map(|l| LineView {
                        y: l.y,
                        height: l.height,
                        header: l.header,
                        cards: l
                            .cards
                            .into_iter()
                            .map(|c| CardView {
                                index: c.row,
                                x: c.x,
                                width: c.width,
                                height: c.height,
                                row: take(c.row, c.att.is_some()),
                                att: c.att.and_then(|id| attach(c.key, id)),
                            })
                            .collect(),
                    })
                    .collect();
                Reply::Lines {
                    view,
                    total_height,
                    len,
                    focus,
                    lines,
                }
            }
            Query::Pass => Reply::Pass {
                pass: self.pass_view(),
            },
            Query::Project => Reply::Project {
                project: self.project_state(),
            },
            Query::Commit => Reply::Commit {
                commit: self.commit_view(),
            },
            Query::FilePatch { mod_name, path } => Reply::FilePatch {
                file: self.file_patch_view(&mod_name, &path),
            },
            Query::Sync => {
                let (plan, extras) = self.commit_plan();
                Reply::Sync {
                    records: self.record_views(),
                    plan: Box::new(plan),
                    extras,
                }
            }
            Query::Tags => Reply::Tags {
                tags: self
                    .session
                    .book()
                    .tags_in_use()
                    .into_iter()
                    .map(|(name, entries)| TagView {
                        color: self.tags.color(&name),
                        name,
                        entries,
                    })
                    .collect(),
                catalog: self.tags.clone(),
            },
            Query::RecordContexts => Reply::RecordContexts {
                contexts: self.record_contexts(),
            },
            Query::RecordTable {
                context,
                row_start,
                row_count,
                col_start,
                col_count,
            } => Reply::ContextTable {
                table: self
                    .record_table(
                        context,
                        (row_start, row_count.min(500)),
                        (col_start, col_count.min(1000)),
                    )
                    .map(Box::new),
            },
            Query::StackTargets {
                domain,
                key,
                context,
                chain,
                stack,
                offset,
                limit,
            } => {
                let (targets, total) = self
                    .stack_targets(&domain, key, context, chain, stack, (offset, limit.min(2000)))
                    .unwrap_or_default();
                Reply::StackTargets { targets, total }
            }
            Query::ConflictDomains => Reply::Domains {
                domains: self.conflict_domains(),
            },
            Query::DomainContexts { domain } => Reply::DomainContexts {
                contexts: self.domain_contexts(&domain),
            },
            Query::DomainCards { domain, context } => Reply::ChainCards {
                cards: self.domain_cards(&domain, context),
            },
            Query::DomainCard {
                domain,
                context,
                index,
                limit,
            } => Reply::ChainCard {
                card: self
                    .domain_card(&domain, context, index, limit.min(5000))
                    .map(Box::new),
            },
            Query::DomainTable {
                domain,
                context,
                row_start,
                row_count,
                col_start,
                col_count,
            } => Reply::ContextTable {
                table: self
                    .domain_table(
                        &domain,
                        context,
                        (row_start, row_count.min(400)),
                        (col_start, col_count.min(2000)),
                    )
                    .map(Box::new),
            },
            Query::ContextCards { key, last, skip } => Reply::ChainCards {
                cards: self.context_cards(key, last.as_deref(), &skip),
            },
            Query::OrderAdvice {
                key,
                strictness,
                current_truth,
                locked,
            } => Reply::OrderAdvice {
                advice: self.order_advice(key, strictness, current_truth, &locked),
            },
            Query::ChainOrphans => Reply::ChainOrphans {
                orphans: self.orphans(),
            },
            Query::FileTree {
                subject,
                mode,
                focus,
                opened,
                closed,
                limit,
            } => Reply::FileTree {
                tree: self.file_tree(
                    &subject,
                    mode,
                    focus.as_deref(),
                    &opened,
                    &closed,
                    limit.min(20_000),
                ),
            },
            Query::PathOwners { path } => Reply::PathOwners {
                owners: self.path_owners(&path),
            },
            Query::FilePreview { path, copy } => Reply::FilePreview {
                file: self.file_preview(&path, copy).map(Box::new),
            },
            Query::VfsDir { dir } => Reply::VfsDir {
                entries: self.vfs_dir(&dir).unwrap_or_default(),
            },
            Query::FindPaths { text, limit } => {
                let (paths, total) = self.find_paths(&text, limit.min(5000));
                Reply::FoundPaths { paths, total }
            }
            Query::PinnedFiles => Reply::PinnedFiles { files: self.pinned() },
            Query::Places => Reply::Places {
                places: self
                    .preview_source
                    .0
                    .as_ref()
                    .map(|s| s.places())
                    .unwrap_or_default(),
            },
            Query::PlaceDir { place, dir } => Reply::PlaceDir {
                entries: self
                    .preview_source
                    .0
                    .as_ref()
                    .and_then(|s| s.list(&place, &dir))
                    .unwrap_or_default(),
            },
            Query::PlaceFile { place, path } => Reply::PlaceFile {
                file: self.place_file(&place, &path).map(Box::new),
            },
            Query::Asset { path } => Reply::Asset {
                asset: self.asset(&path).map(Box::new),
            },
            Query::SkeletonFor { path } => Reply::Asset {
                asset: self.skeleton_for(&path).map(Box::new),
            },
            Query::BodyFor { path } => Reply::Assets {
                assets: self.body_for(&path),
            },
            Query::Thumb { path, place, side } => Reply::Thumb {
                picture: self.thumb(&path, place.as_deref(), side),
            },
            Query::ContextCard { key, index, limit } => Reply::ChainCard {
                card: self.context_card(key, index, limit.min(5000)).map(Box::new),
            },
            Query::RecordCards { context } => Reply::ChainCards {
                cards: self.record_cards(context),
            },
            Query::RecordCard {
                context,
                index,
                limit,
            } => Reply::ChainCard {
                card: self.record_card(context, index, limit.min(5000)).map(Box::new),
            },
            Query::Contexts => Reply::Contexts {
                contexts: self.contexts(),
            },
            Query::ContextTable {
                key,
                order,
                focus,
                only,
                row_start,
                row_count,
                col_start,
                col_count,
            } => Reply::ContextTable {
                table: self
                    .context_table(
                        key,
                        order,
                        focus,
                        only,
                        (row_start, row_count),
                        (col_start, col_count),
                    )
                    .map(Box::new),
            },
            // @sat:app.decoration
            Query::Indicators => Reply::Indicators {
                indicators: self.indicator_values(),
                defs: if self.indicator_defs.is_empty() {
                    sat_core::counters::default_indicators()
                } else {
                    self.indicator_defs.clone()
                },
            },
            Query::Buffer => {
                let book = self.session.book();
                let areas = book
                    .buffer()
                    .iter()
                    .map(|(n, ks)| BufferAreaView {
                        name: n.clone(),
                        members: ks
                            .iter()
                            .map(|&k| BufferMember {
                                key: k,
                                name: book.name(k).to_string(),
                                enabled: book.enabled(k),
                            })
                            .collect(),
                    })
                    .collect();
                Reply::Buffer { areas }
            }
            Query::Selection { view } => Reply::Selection {
                selection: self.selection_view(view),
            },
            Query::Decoration { view } => {
                let book = self.session.book();
                let keys: Vec<Key> = book
                    .selection(view)
                    .into_iter()
                    .filter(|&k| book.kind(k) == sat_core::canon::EntryKind::Mod)
                    .collect();
                let mut at: std::collections::HashMap<Option<&CardStyle>, usize> = Default::default();
                let mut styles: Vec<StyleShare> = Vec::new();
                for &k in &keys {
                    let st = book.style(k);
                    let i = *at.entry(st).or_insert_with(|| {
                        styles.push(StyleShare {
                            style: st.cloned(),
                            count: 0,
                        });
                        styles.len() - 1
                    });
                    styles[i].count += 1;
                }
                Reply::Decoration { keys, styles }
            }
            Query::Separators => {
                let book = self.session.book();
                let mut tiles: Vec<SeparatorTile> = Vec::new();
                for (pos, (name, kind, enabled)) in book.entries().enumerate() {
                    match kind {
                        sat_core::canon::EntryKind::Separator => tiles.push(SeparatorTile {
                            key: book.key_at(pos),
                            name: name.trim_end_matches("_separator").to_string(),
                            index: tiles.len() as u32 + 1,
                            first: None,
                            mods: 0,
                            enabled: 0,
                            level: book.level(book.key_at(pos)),
                        }),
                        sat_core::canon::EntryKind::Mod => {
                            if let Some(t) = tiles.last_mut() {
                                t.first.get_or_insert(pos as u32);
                                t.mods += 1;
                                t.enabled += u32::from(enabled);
                            }
                        }
                        sat_core::canon::EntryKind::Foreign => {}
                    }
                }
                Reply::Separators { tiles }
            }
            Query::Keymap => Reply::Keymap {
                bindings: self
                    .registry
                    .keymap()
                    .bindings()
                    .filter(|(ctx, _, _)| *ctx == "list")
                    .map(|(_, ch, cmd)| (ch.to_string(), cmd.to_string()))
                    .collect(),
            },
        })
    }
}

// @sat:app.badges.normalized
/// Context badges of one mod: its normalized chains grouped by context, largest first, `limit` icons per context.
fn mod_badges(
    ow: &sat_core::overwrite::OverwriteProjection,
    heads: &chains::Heads,
    session: &sat_core::session::Session,
    m: ModId,
    limit: u8,
) -> Vec<ContextBadge> {
    let decisions = session.decisions();
    let parts = chains::parts(ow, heads, m);
    if parts.is_empty() {
        return Vec::new();
    }
    let mut by_ctx: Vec<(Option<sat_core::ids::ContextId>, Vec<chains::Part>)> = Vec::new();
    for p in parts {
        let ctx = ow.chain_context(heads[p.group as usize].chains[0]);
        match by_ctx.iter_mut().find(|(k, _)| *k == ctx) {
            Some((_, v)) => v.push(p),
            None => by_ctx.push((ctx, vec![p])),
        }
    }
    by_ctx
        .into_iter()
        .map(|(ctx, ps)| {
            let hue = ctx.map_or(210, |k| {
                sat_core::identity::identity(
                    ow.contexts()[k.idx()].mods.iter().map(|&x| session.member_key(x)),
                )
                .hue
            });
            let shown: Vec<ChainBadge> = ps
                .iter()
                .take(limit as usize)
                .map(|p| {
                    let h = &heads[p.group as usize];
                    ChainBadge {
                        signature: h.hex(),
                        done: h.done(decisions),
                        emoji: h.emoji.to_string(),
                        hue: h.hue,
                        files: p.files,
                        place: p.place.min(255) as u8,
                        stack: h
                            .route
                            .iter()
                            .map(|&x| session.unit_label(x).to_string())
                            .collect(),
                    }
                })
                .collect();
            ContextBadge {
                hue,
                more: (ps.len() - shown.len()) as u32,
                files: ps.iter().map(|p| p.files).sum(),
                chains: shown,
            }
        })
        .collect()
}

const CHAIN_LIMIT: usize = 60;
const SAMPLE_LIMIT: usize = 6;

impl App {
    fn mod_detail(&mut self, key: Key) -> ModDetail {
        let ow = self.session.overwrite();
        let plugins = self.session.plugins();
        let table = self.session.table().clone();
        let book = self.session.book();
        let name = book.name(key).to_string();
        let index: std::collections::HashMap<String, ModId> = table
            .mod_names()
            .iter()
            .enumerate()
            .map(|(i, n)| (n.to_lowercase(), ModId::from_idx(i)))
            .collect();
        let Some(&m) = index.get(&name.to_lowercase()) else {
            return ModDetail {
                key,
                name,
                counters: None,
                opponents: Vec::new(),
                chains: Vec::new(),
                more_chains: 0,
                plugins: Vec::new(),
                marks: self.session.book().marks(key).cloned(),
            };
        };
        // a member of a group computed as one: the group speaks for it in conflicts; its plugins are its own
        let own = m;
        let m = ow.units().rep(m);
        // opponents over every exact chain (exact counts); the list and the teeth over normalized chains
        let chains = ow.mod_chains(m);
        let mut opp: std::collections::HashMap<ModId, (u32, u32)> = Default::default();
        for &c in &chains {
            let stack = ow.chain_mods(c);
            let me = stack.iter().position(|&x| x == m).unwrap_or(0);
            let files = ow.chain(c).files;
            for (i, &o) in stack.iter().enumerate() {
                if o == m {
                    continue;
                }
                let e = opp.entry(o).or_default();
                if i < me {
                    e.0 += files;
                } else {
                    e.1 += files;
                }
            }
        }
        let heads = self.heads();
        let parts = chains::parts(&ow, &heads, m);
        let more_chains = parts.len().saturating_sub(CHAIN_LIMIT);
        let listed: Vec<&chains::Head> = parts
            .iter()
            .take(CHAIN_LIMIT)
            .map(|p| &heads[p.group as usize])
            .collect();
        let chain_views: Vec<ChainView> = parts
            .iter()
            .take(CHAIN_LIMIT)
            .map(|p| {
                let h = &heads[p.group as usize];
                // sample paths where this mod takes part
                let samples: Vec<String> = chains
                    .iter()
                    .filter(|&&c| heads.of_chain(c) == Some(p.group))
                    .flat_map(|&c| ow.chain_paths(c))
                    .take(SAMPLE_LIMIT)
                    .map(|q| table.path_string(q))
                    .collect();
                ChainView {
                    id: p.group,
                    signature: h.hex(),
                    done: h.done(self.session.decisions()),
                    emoji: h.emoji.to_string(),
                    hue: h.hue,
                    title: h.title.clone(),
                    name: h.name.clone(),
                    custom: h.custom,
                    content: chains::chain_content(&table, &self.content_hashes, &ow, &h.chains),
                    files: h.files,
                    bytes: h.bytes,
                    stack: h
                        .route
                        .iter()
                        .map(|&x| self.session.unit_label(x).to_string())
                        .collect(),
                    place: p.place,
                    samples,
                }
            })
            .collect();
        opp.entry(m).or_default();
        let book = self.session.book();
        let mut opponents: Vec<Opponent> = opp
            .into_iter()
            .map(|(o, (beats_me, i_beat))| {
                let oname = table.mod_name(o).to_string();
                let okey = book.key_of(&oname);
                let teeth = listed
                    .iter()
                    .map(|h| match h.route.iter().position(|&x| x == o) {
                        Some(0) => 'w',
                        Some(_) => 'l',
                        None => '-',
                    })
                    .collect();
                Opponent {
                    key: okey,
                    pos: okey.map(|k| book.pos(k) as u32),
                    separator: okey
                        .and_then(|k| book.separator_of(k))
                        .map(|s| book.name(s).trim_end_matches("_separator").to_string()),
                    me: o == m,
                    enabled: ow.priorities().is_active(o),
                    name: oname,
                    beats_me,
                    i_beat,
                    teeth,
                }
            })
            .collect();
        opponents.sort_by_key(|o| (o.pos.is_none(), o.pos, o.name.clone()));
        let s = ow.stat(m);
        let mod_plugins: Vec<PluginView> = plugins
            .plugins
            .iter()
            .filter(|p| p.versions.iter().any(|v| v.owner == own))
            .map(|p| PluginView {
                name: p.name.clone(),
                index: p.index.map(|i| i.to_string()),
                colour: p.colour,
                listed: p.listed,
                active: p.active,
                owner: p.winner.map(|w| table.mod_name(p.versions[w].owner).to_string()),
                versions: p.versions.len(),
                missing_masters: p.missing_masters.clone(),
                records: self.plugin_records(&p.name),
            })
            .collect();
        ModDetail {
            key,
            name,
            counters: Some(Counters {
                win: s.win,
                lose: s.lose,
                mid: s.mid,
                unique: s.unique,
                x: s.fully_overridden,
                plugins: mod_plugins.len() as u32,
            }),
            opponents,
            chains: chain_views,
            more_chains,
            plugins: mod_plugins,
            marks: self.session.book().marks(key).cloned(),
        }
    }
}

/// The inside of a logical group: what the outside does not see (PHILOSOPHY «Лог-группа»).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupInside {
    pub name: String,
    pub members: Vec<String>,
    /// Paths the members fight over among themselves.
    pub conflict_paths: u32,
    /// The largest inner chains, winner first.
    pub chains: Vec<InsideChain>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InsideChain {
    pub stack: Vec<String>,
    pub files: u32,
}

/// The main view (always open).
pub const MAIN_VIEW: ViewId = MAIN;

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use sat_core::canon::{Entry, EntryKind, Modlist};
    use sat_core::order::OrderBook;
    use sat_core::table::ProviderTable;

    fn app() -> App {
        app_of(["A", "B"])
    }

    pub(crate) fn app_of(mods: [&str; 2]) -> App {
        let facts: Vec<sat_core::facts::ModFacts> = mods
            .iter()
            .map(|n| {
                let mut f = sat_core::facts::ModFacts {
                    name: n.to_string(),
                    ..Default::default()
                };
                f.dirs.push(sat_core::facts::LooseDir {
                    rel: "textures".into(),
                    files: vec![sat_core::facts::LooseFile {
                        name: "a.dds".into(),
                        size: 10,
                        vram: 0,
                    }],
                });
                f.metrics.bytes = 10;
                f.metrics.files = 1;
                f
            })
            .collect();
        app_from(facts)
    }

    /// An app over these mods, in this order, under one separator.
    pub(crate) fn app_from(facts: Vec<sat_core::facts::ModFacts>) -> App {
        let table = Arc::new(ProviderTable::build(&facts));
        let names: Vec<String> = std::iter::once("S_separator".to_string())
            .chain(facts.iter().map(|f| f.name.clone()))
            .collect();
        let list = Modlist::new(
            names
                .iter()
                .map(|n| Entry {
                    name: n.to_string(),
                    kind: if n.ends_with("_separator") {
                        EntryKind::Separator
                    } else {
                        EntryKind::Mod
                    },
                    enabled: true,
                })
                .collect(),
        );
        App::new(
            Session::new(OrderBook::from_modlist(&list), table),
            Registry::with_builtins(),
        )
    }

    #[test]
    fn find_and_rule_stickers_read_the_same_metrics() {
        let mut a = app();
        let find = |a: &mut App, e: &str| match a
            .query(Query::Find {
                expr: e.into(),
                limit: 10,
            })
            .unwrap()
        {
            Reply::Found { found } => found
                .keys
                .iter()
                .map(|&k| a.session.book().name(k).to_string())
                .collect::<Vec<_>>(),
            _ => panic!(),
        };
        assert_eq!(find(&mut a, "weight >= 10"), ["A", "B"]);
        // B is lower in the list: it wins the shared file
        assert_eq!(find(&mut a, "wins > 0"), ["B"]);
        assert!(matches!(
            a.query(Query::Find {
                expr: "size >".into(),
                limit: 1
            }),
            Err(AppError::Expr(_))
        ));
        let bad = a.set_sticker_rules(&[
            sat_core::sticker_rules::StickerRule {
                sticker: "hidden".into(),
                when: "loses > 0".into(),
            },
            sat_core::sticker_rules::StickerRule {
                sticker: "x".into(),
                when: "nonsense_metric > 1".into(),
            },
        ]);
        assert_eq!(bad.len(), 1, "{bad:?}");
        let key = a.session.book().key_of("A").unwrap();
        let Reply::ModSheet { sheet } = a
            .query(Query::ModSheet {
                key,
                level: sheet::SheetLevel::Full,
            })
            .unwrap()
        else {
            panic!()
        };
        // where a mod comes from: a known update is a metric and a sheet section
        a.set_sources(vec![(
            "B".into(),
            sat_core::source::ModSource {
                site: "nexus".into(),
                game: Some("SkyrimSE".into()),
                mod_id: Some(7),
                version: Some("1.0".into()),
                newest_version: Some("1.1".into()),
                ..Default::default()
            },
        )]);
        assert_eq!(find(&mut a, "update && nexus_id == 7"), ["B"]);
        let kb = a.session.book().key_of("B").unwrap();
        let Reply::ModSheet { sheet: sb } = a
            .query(Query::ModSheet {
                key: kb,
                level: sheet::SheetLevel::Full,
            })
            .unwrap()
        else {
            panic!()
        };
        let src = sb.sections.iter().find(|s| s.id == "source").unwrap();
        assert_eq!(src.fields[0].id, "update");
        assert!(src.fields.iter().any(|f| f.value
            == sheet::Value::Text {
                text: "https://www.nexusmods.com/skyrimspecialedition/mods/7".into()
            }));
        // the counter constructor: rows carry the project's counters
        assert!(
            a.set_counters(&[sat_core::counters::CounterDef {
                id: "wl".into(),
                template: "{wins}/{loses}".into(),
                cases: vec![sat_core::counters::Case {
                    when: "loses > 0".into(),
                    tone: "bad".into(),
                }],
                show_if: String::new(),
            }])
            .is_empty()
        );
        let Reply::Rows { rows, .. } = a
            .query(Query::Rows {
                view: 0,
                start: 0,
                count: 10,
                badges: 0,
            })
            .unwrap()
        else {
            panic!()
        };
        let a_row = rows.iter().find(|r| r.name == "A").unwrap();
        assert_eq!(a_row.custom[0].text, "0/1");
        assert_eq!(a_row.custom[0].tone.as_deref(), Some("bad"));
        let rules = sheet.sections.iter().find(|s| s.id == "rules").unwrap();
        assert_eq!(
            rules.fields[0].value,
            sheet::Value::Names {
                items: vec!["hidden".into()]
            }
        );
    }

    #[test]
    fn a_mod_sheet_has_three_depths_and_signals_first() {
        use sheet::{Origin, SheetLevel, Signal, Value};
        let mut a = app();
        a.session.set_member_keys([("A".to_string(), "e1".to_string())]);
        let key = a.session.book().key_of("A").unwrap();
        let sheet = |a: &mut App, level| match a.query(Query::ModSheet { key, level }).unwrap() {
            Reply::ModSheet { sheet } => *sheet,
            _ => panic!(),
        };
        // A is above B: its only file loses — nothing of it is seen in the game
        let full = sheet(&mut a, SheetLevel::Full);
        assert_eq!(
            (
                full.name.as_str(),
                full.entity.as_deref(),
                full.separator.as_deref()
            ),
            ("A", Some("e1"), Some("S"))
        );
        assert_eq!(full.signals[0].id, "fully_overridden");
        assert_eq!(full.signals[0].signal, Some(Signal::Warn));
        // a mod without plugins, found tags or rule stickers: those sections say nothing and are left out
        let ids: Vec<(&str, Origin)> = full.sections.iter().map(|s| (s.id.as_str(), s.origin)).collect();
        assert_eq!(
            ids,
            [
                ("content", Origin::Observed),
                ("profile", Origin::Observed),
                ("conflicts", Origin::Observed),
                ("user", Origin::User)
            ]
        );
        let profile = &full.sections[1].fields;
        assert!(profile.iter().any(|f| f.id == "package"), "{profile:?}");
        let content = &full.sections[0].fields;
        assert_eq!(content[0].id, "bytes");
        assert_eq!(content[0].value, Value::Bytes { n: 10 });
        assert!(content.iter().all(|f| f.id != "dll"), "zero counts say nothing");
        let conflicts = &full.sections[2].fields;
        assert_eq!(conflicts[0].id, "fully_overridden");
        // the badge: signals only
        let badge = sheet(&mut a, SheetLevel::Badge);
        assert!(badge.sections.is_empty() && badge.signals.len() <= sheet::BADGE_SIGNALS);
        // settling the chain takes its note away
        let head = a.heads()[0].hex();
        a.command(Command::SetChainDone {
            signature: head,
            done: true,
        })
        .unwrap();
        let mini = sheet(&mut a, SheetLevel::Mini);
        assert!(mini.signals.iter().all(|f| f.id != "chains"));
        let json = serde_json::to_string(&mini).unwrap();
        assert!(json.contains("\"origin\":\"observed\""), "{json}");
    }

    #[test]
    fn chains_and_mods_tell_what_the_losing_copies_are() {
        use sheet::SheetLevel;
        let mut a = app_of(["A", "B"]);
        let key = a.session.book().key_of("A").unwrap();
        let detail = |a: &mut App| match a.query(Query::ModDetail { key }).unwrap() {
            Reply::ModDetail { detail } => detail,
            other => panic!("{other:?}"),
        };
        assert_eq!(detail(&mut a).chains[0].content, None, "no hashes: not compared");
        let n = a.session().table().provider_count();
        a.set_content_hashes(sat_core::identical::ContentHashes(vec![Some(7); n]));
        let c = detail(&mut a).chains[0].content.unwrap();
        assert_eq!(
            (c.verdict, c.equal_files, c.equal_bytes),
            (sat_core::identical::Verdict::Identical, 1, 10)
        );
        // still a conflict: counted and shown, with what the copy is
        let Reply::ModSheet { sheet } = a
            .query(Query::ModSheet {
                key,
                level: SheetLevel::Full,
            })
            .unwrap()
        else {
            panic!()
        };
        let conflicts = &sheet
            .sections
            .iter()
            .find(|s| s.id == "conflicts")
            .unwrap()
            .fields;
        assert!(conflicts.iter().any(|f| f.id == "loses"));
        assert!(conflicts.iter().any(|f| f.id == "equal_copies"), "{conflicts:?}");
    }

    #[test]
    fn a_chain_wears_the_identity_the_user_gave_it() {
        let mut a = app_of(["A", "B"]);
        let head = a.heads()[0].clone();
        let set = |a: &mut App, name: Option<&str>, emoji: Option<&str>, hue: Option<u16>| {
            a.command(Command::SetChainIdentity {
                signature: head.hex(),
                name: name.map(String::from),
                emoji: emoji.map(String::from),
                hue,
            })
            .unwrap()
        };
        set(&mut a, Some("My chain"), Some("🧪"), Some(400));
        let h = a.heads()[0].clone();
        assert_eq!(
            (h.name.as_str(), h.emoji.as_str(), h.hue, h.custom),
            ("My chain", "🧪", 40, true)
        );
        // another edit replaces the identity, it does not stack
        set(&mut a, Some("Other"), None, None);
        let h = a.heads()[0].clone();
        assert_eq!((h.name.as_str(), &h.emoji), ("Other", &head.emoji));
        assert_eq!(a.chain_identities().len(), 1);
        // nothing given: the generated identity is back
        assert!(matches!(set(&mut a, None, None, None), Event::Changed { .. }));
        let h = a.heads()[0].clone();
        assert_eq!((h.name, h.custom), (head.name.clone(), false));
        assert!(a.chain_identities().is_empty());
        assert!(matches!(set(&mut a, None, None, None), Event::Nothing));
    }

    #[test]
    fn a_settled_chain_stays_settled_when_its_mod_is_renamed() {
        let keyed = |mods: [&str; 2], ids: [&str; 2]| {
            let mut a = app_of(mods);
            a.session
                .set_member_keys(mods.iter().zip(ids).map(|(n, k)| (n.to_string(), k.to_string())));
            a
        };
        let mut before = keyed(["A", "B"], ["e1", "e2"]);
        let head = before.heads()[0].clone();
        before
            .command(Command::SetChainDone {
                signature: head.hex(),
                done: true,
            })
            .unwrap();
        let settled = before.session.decisions().clone();
        // the project reopened after `A` was renamed: same entity, new folder name
        let mut after = keyed(["A v2", "B"], ["e1", "e2"]);
        after.session.set_decisions(settled.clone());
        let h = after.heads()[0].clone();
        assert_eq!((h.signature, &h.emoji), (head.signature, &head.emoji));
        assert!(h.done(after.session.decisions()));
        // without entity ids the folder name is the key: the renamed chain is a new one
        let mut unkeyed = app_of(["A v2", "B"]);
        unkeyed.session.set_decisions(settled);
        assert!(!unkeyed.heads()[0].done(unkeyed.session.decisions()));
    }

    #[test]
    fn json_round_trip_and_keys_drive_the_list() {
        let mut a = app();
        let c: Command = serde_json::from_str(r#"{"type":"key","view":0,"chord":"ArrowDown"}"#).unwrap();
        assert!(matches!(
            a.command(c).unwrap(),
            Event::Changed { canon: false, .. }
        ));
        a.command(Command::Key {
            view: 0,
            chord: "ArrowDown".into(),
        })
        .unwrap(); // aim at A (the cursor does not select, ADR 0020 §4)
        a.command(Command::Key {
            view: 0,
            chord: "Space".into(),
        })
        .unwrap(); // A into the selection
        let ev = a
            .command(Command::Key {
                view: 0,
                chord: "Alt+ArrowDown".into(),
            })
            .unwrap();
        assert!(matches!(ev, Event::Changed { canon: true, .. }));
        let Reply::Rows { rows, len, .. } = a
            .query(Query::Rows {
                view: 0,
                start: 0,
                count: 10,
                badges: 4,
            })
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(len, 3);
        assert_eq!(
            rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            ["S_separator", "B", "A"]
        );
        // A is now below B = higher priority: it wins the shared texture
        let a_row = &rows[2];
        assert_eq!(a_row.counters.unwrap().win, 1);
        // one context, one chain icon; the same chain shows the same icon on both mods
        assert_eq!(a_row.badges.len(), 1);
        let icon = &a_row.badges[0].chains[0];
        assert_eq!((icon.place, icon.files, icon.stack.len()), (0, 1, 2));
        assert_eq!(rows[1].badges[0].chains[0].emoji, icon.emoji);
        assert_eq!(rows[1].badges[0].chains[0].place, 1);
        assert!(a_row.focused && a_row.selected);
        let json = serde_json::to_string(&a.query(Query::Totals).unwrap()).unwrap();
        assert!(json.contains("\"conflict_paths\":1"), "{json}");
        assert!(matches!(
            a.command(Command::Key {
                view: 0,
                chord: "F9".into()
            }),
            Err(AppError::Unbound(_))
        ));
        let key_a = rows[2].key;
        let Reply::ModDetail { detail } = a.query(Query::ModDetail { key: key_a }).unwrap() else {
            panic!()
        };
        // participants: B, then A itself (list order), grouped under separator S
        let names: Vec<&str> = detail.opponents.iter().map(|o| o.name.as_str()).collect();
        assert_eq!(names, ["B", "A"]);
        let b = &detail.opponents[0];
        assert_eq!(
            (
                b.i_beat,
                b.beats_me,
                b.teeth.as_str(),
                b.me,
                b.separator.as_deref()
            ),
            (1, 0, "l", false, Some("S"))
        );
        assert_eq!(detail.opponents[1].teeth, "w");
        assert!(detail.opponents[1].me);
        let ch = &detail.chains[0];
        assert_eq!(ch.title, "textures · dds");
        assert_eq!(ch.name, "textures · dds · B → A");
        assert_eq!(ch.stack, ["A", "B"]);
        // settle the chain: the badge shows it on both mods, and it survives a move
        assert!(matches!(
            a.command(Command::SetChainDone {
                signature: ch.signature.clone(),
                done: true
            })
            .unwrap(),
            Event::Decided
        ));
        let b_key = b.key.unwrap();
        a.command(Command::Move {
            keys: vec![key_a],
            before: Some(b_key),
            after: None,
        })
        .unwrap();
        let Reply::Rows { rows: after, .. } = a
            .query(Query::Rows {
                view: 0,
                start: 0,
                count: 10,
                badges: 4,
            })
            .unwrap()
        else {
            panic!()
        };
        assert!(
            after
                .iter()
                .filter(|r| r.kind == RowKind::Mod)
                .all(|r| r.badges[0].chains[0].done)
        );
        assert!(matches!(
            a.command(Command::SetChainDone {
                signature: "zz".into(),
                done: true
            }),
            Err(AppError::Signature(_))
        ));
        // a link from the summary moves the focus
        a.command(Command::Focus {
            view: 0,
            key: b.key.unwrap(),
        })
        .unwrap();
        let Reply::Rows { focus_row, .. } = a
            .query(Query::Rows {
                view: 0,
                start: 0,
                count: 1,
                badges: 0,
            })
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(focus_row, Some(2), "B is below A after the move");
    }

    #[test]
    fn the_buffer_holds_areas_and_places_them() {
        let mut a = app();
        let (ka, kb) = {
            let b = a.session().book();
            (b.key_of("A").unwrap(), b.key_of("B").unwrap())
        };
        let buffer = |a: &mut App| match a.query(Query::Buffer).unwrap() {
            Reply::Buffer { areas } => areas,
            _ => unreachable!(),
        };
        a.command(Command::Buffer {
            edit: BufferEdit::Put {
                keys: vec![ka],
                area: None,
            },
        })
        .unwrap();
        a.command(Command::Buffer {
            edit: BufferEdit::Put {
                keys: vec![kb],
                area: Some(0),
            },
        })
        .unwrap();
        let areas = buffer(&mut a);
        assert_eq!(areas.len(), 1);
        assert_eq!(
            areas[0]
                .members
                .iter()
                .map(|m| m.name.as_str())
                .collect::<Vec<_>>(),
            ["A", "B"]
        );
        assert_eq!(
            a.session().book().decor().buffer[0].members,
            ["A", "B"],
            "kept in the project by name"
        );
        // the list did not change: the buffer holds references
        let order = |a: &mut App| {
            let b = a.session().book();
            (b.pos(ka), b.pos(kb))
        };
        let (pa, pb) = order(&mut a);
        assert!(pa < pb);
        // place the area at the end of the list
        a.command(Command::Buffer {
            edit: BufferEdit::Place {
                area: 0,
                before: None,
            },
        })
        .unwrap();
        assert!(buffer(&mut a).is_empty(), "a placed area goes");
        let (pa2, pb2) = order(&mut a);
        assert_eq!(
            (pa2, pb2),
            (pb - 1, pb),
            "at the end of the list, in the area's order"
        );
        // selection summary
        a.command(Command::Navigate {
            view: order::MAIN,
            nav: order::Nav::Row(1),
            select: order::Select::Replace,
        })
        .unwrap();
        let Reply::Selection { selection } = a.query(Query::Selection { view: order::MAIN }).unwrap() else {
            panic!()
        };
        assert_eq!(selection.count, 1);
        assert_eq!(selection.names.len(), 1);
    }

    #[test]
    fn groups_fold_open_with_an_outline_and_dissolve() {
        let mut a = app();
        let (ka, kb) = {
            let b = a.session().book();
            (b.key_of("A").unwrap(), b.key_of("B").unwrap())
        };
        a.command(Command::CreateGroup {
            name: "Fam".into(),
            keys: vec![ka, kb],
        })
        .unwrap();
        let rows = |a: &mut App| match a
            .query(Query::Rows {
                view: order::MAIN,
                start: 0,
                count: 10,
                badges: 0,
            })
            .unwrap()
        {
            Reply::Rows { rows, .. } => rows,
            _ => unreachable!(),
        };
        let r = rows(&mut a);
        assert_eq!(r.len(), 2, "a collapsed group is one row");
        assert_eq!((r[1].kind, r[1].name.as_str()), (RowKind::Group, "Fam"));
        a.command(Command::SetGroupCollapsed {
            view: order::MAIN,
            key: kb,
            collapsed: false,
        })
        .unwrap();
        let r = rows(&mut a);
        let marks: Vec<_> = r.iter().filter_map(|r| r.group.clone()).collect();
        assert_eq!(marks.len(), 2, "open: both members, outlined");
        assert!(marks[0].first && marks[1].last && marks[0].size == 2);
        a.command(Command::DissolveGroup { key: ka }).unwrap();
        assert!(rows(&mut a).iter().all(|r| r.group.is_none()));
        a.command(Command::Undo).unwrap();
        assert!(
            rows(&mut a).iter().any(|r| r.group.is_some()),
            "dissolving is one undo step"
        );
    }

    #[test]
    fn attached_elements_come_with_their_anchor_in_lines() {
        let mut a = app();
        let k = a.session().book().key_of("A").unwrap();
        a.command(Command::EditMarks {
            keys: vec![k],
            edit: MarksEdit::AddTag { tag: "3BA".into() },
        })
        .unwrap();
        for edit in [
            AttachEdit::Add {
                kind: order::AttachKind::Note { text: "check".into() },
                width: None,
                height: None,
            },
            AttachEdit::Add {
                kind: order::AttachKind::Summary { tag: "3ba".into() },
                width: None,
                height: None,
            },
        ] {
            a.command(Command::Attach { key: k, edit }).unwrap();
        }
        let flow = FlowParams {
            width: 2000,
            card_width: 240,
            card_height: 40,
            header_height: 30,
            gap: 4,
            reserve: 0,
            natural: None,
        };
        let lines = |a: &mut App| {
            let Reply::Lines { lines, len, .. } = a
                .query(Query::Lines {
                    view: order::MAIN,
                    y: 0,
                    height: 10_000,
                    flow: flow.clone(),
                    badges: 3,
                    measures: None,
                })
                .unwrap()
            else {
                panic!()
            };
            assert_eq!(len, 3, "elements are not rows");
            lines.into_iter().flat_map(|l| l.cards).collect::<Vec<_>>()
        };
        let cards = lines(&mut a);
        let atts: Vec<&CardView> = cards.iter().filter(|c| c.att.is_some()).collect();
        assert_eq!(atts.len(), 2);
        assert!(atts.iter().all(|c| c.row.key == k), "they carry the anchor's row");
        let ids: Vec<u32> = atts
            .iter()
            .map(|c| c.att.as_ref().unwrap().attachment.id)
            .collect();
        assert_eq!(ids, vec![1, 2], "ids are given in turn");
        assert_eq!(
            atts[1].att.as_ref().unwrap().summary,
            Some(FamilyCount { total: 1, enabled: 1 }),
            "the family summary counts the tag case-folded"
        );
        a.command(Command::Attach {
            key: k,
            edit: AttachEdit::Remove { id: 1 },
        })
        .unwrap();
        assert_eq!(lines(&mut a).iter().filter(|c| c.att.is_some()).count(), 1);
        a.command(Command::Undo).unwrap();
        assert_eq!(
            lines(&mut a).iter().filter(|c| c.att.is_some()).count(),
            2,
            "one undo step"
        );
    }

    #[test]
    fn lines_place_every_card_once_and_follow_styles() {
        let mut a = app();
        let flow = FlowParams {
            width: 500,
            card_width: 240,
            card_height: 40,
            header_height: 30,
            gap: 4,
            reserve: 0,
            natural: None,
        };
        let q = Query::Lines {
            view: order::MAIN,
            y: 0,
            height: 10_000,
            flow,
            badges: 3,
            measures: None,
        };
        let Reply::Lines {
            lines,
            total_height,
            len,
            ..
        } = a.query(q.clone()).unwrap()
        else {
            panic!()
        };
        assert_eq!(len, 3);
        // header + one line with A and B side by side
        assert_eq!(lines.len(), 2);
        assert!(lines[0].header && lines[0].cards[0].row.name == "S_separator");
        assert_eq!(
            lines[1]
                .cards
                .iter()
                .map(|c| c.row.name.as_str())
                .collect::<Vec<_>>(),
            ["A", "B"]
        );
        assert!(
            lines[1].cards[0].row.counters.is_some(),
            "cards carry the row view"
        );
        assert_eq!(total_height, 30 + 44);
        // a full-width style breaks the line; the cached index is rebuilt on the new revision
        let b = lines[1].cards[1].row.key;
        a.command(Command::SetStyle {
            keys: vec![b],
            style: Some(CardStyle {
                full_width: true,
                ..CardStyle::default()
            }),
        })
        .unwrap();
        let Reply::Lines {
            lines, total_height, ..
        } = a.query(q).unwrap()
        else {
            panic!()
        };
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[2].cards[0].width, 500);
        assert_eq!(total_height, 30 + 44 + 44);
    }

    #[test]
    fn patch_style_changes_one_field_on_many_and_undoes_in_one_step() {
        let mut a = app();
        let Reply::Rows { rows, .. } = a
            .query(Query::Rows {
                view: order::MAIN,
                start: 0,
                count: 3,
                badges: 0,
            })
            .unwrap()
        else {
            panic!()
        };
        let (ka, kb) = (rows[1].key, rows[2].key);
        a.command(Command::SetStyle {
            keys: vec![ka],
            style: Some(CardStyle {
                width: Some(300),
                ..CardStyle::default()
            }),
        })
        .unwrap();
        let patch = serde_json::json!({"fill": {"color": 0xff0000}, "break_after": true});
        a.command(Command::PatchStyle {
            keys: vec![ka, kb],
            patch,
        })
        .unwrap();
        let book = a.session().book();
        let (sa, sb) = (book.style(ka).cloned().unwrap(), book.style(kb).cloned().unwrap());
        assert_eq!(sa.width, Some(300), "other fields stay");
        assert!(sa.break_after && sb.break_after);
        assert_eq!(
            sb.fill.map(|f| (f.kind, f.color)),
            Some((order::FillKind::Solid, 0xff0000))
        );
        // clearing every field leaves no style
        a.command(Command::PatchStyle {
            keys: vec![kb],
            patch: serde_json::json!({"fill": null, "break_after": null}),
        })
        .unwrap();
        assert!(a.session().book().style(kb).is_none());
        a.command(Command::Undo).unwrap();
        a.command(Command::Undo).unwrap();
        let book = a.session().book();
        assert_eq!(book.style(ka).and_then(|s| s.width), Some(300));
        assert!(!book.style(ka).unwrap().break_after && book.style(kb).is_none());
        // a patch that does not make a style is refused and changes nothing
        let bad = a.command(Command::PatchStyle {
            keys: vec![ka],
            patch: serde_json::json!({"width": "wide"}),
        });
        assert!(matches!(bad, Err(AppError::Style(_))));
    }

    #[test]
    fn plugin_edits_go_through_commands() {
        use sat_core::facts::{ModFacts, PluginFacts};
        use sat_core::plugins::{PluginCanon, PluginVersions};
        let pf = |n: &str| PluginFacts {
            file_name: n.into(),
            header: None,
            records: None,
            error: None,
        };
        let mods = vec![
            ModFacts {
                name: "On".into(),
                plugins: vec![pf("A.esp")],
                ..Default::default()
            },
            ModFacts {
                name: "Off".into(),
                plugins: vec![pf("D.esp")],
                ..Default::default()
            },
        ];
        let list = Modlist::new(
            [("On", true), ("Off", false)]
                .iter()
                .map(|(n, e)| Entry {
                    name: n.to_string(),
                    kind: EntryKind::Mod,
                    enabled: *e,
                })
                .collect(),
        );
        let mut session = Session::new(
            OrderBook::from_modlist(&list),
            Arc::new(ProviderTable::build(&mods)),
        );
        session.set_plugin_facts(Arc::new(PluginVersions::build(&mods)));
        session.set_plugin_canon(PluginCanon::default());
        let mut a = App::new(session, Registry::with_builtins());
        let state = |a: &mut App, n: &str| {
            let p = a.session().plugins();
            let p = p.get(n).unwrap();
            (p.present, p.active, p.blacklisted)
        };
        assert_eq!(state(&mut a, "D.esp"), (false, false, false));
        // checking a plugin of a disabled mod enables the mod
        let ev = a
            .command(Command::SetPluginChecked {
                name: "D.esp".into(),
                checked: true,
            })
            .unwrap();
        assert!(matches!(ev, Event::Changed { canon: true, ref views, .. } if !views.is_empty()));
        assert_eq!(state(&mut a, "D.esp"), (true, true, false));
        a.command(Command::SetPluginHidden {
            name: "A.esp".into(),
            hidden: true,
        })
        .unwrap();
        assert_eq!(state(&mut a, "A.esp"), (true, false, true));
        a.command(Command::SetPluginChecked {
            name: "D.esp".into(),
            checked: false,
        })
        .unwrap();
        a.command(Command::CascadePlugins).unwrap();
        assert!(state(&mut a, "D.esp").1, "cascade turns it back on: no masters");
        assert!(!state(&mut a, "A.esp").1, "hidden stays off");
        // a move to the end changes the order; a base master does not move
        let pos = |a: &mut App, n: &str| a.session().plugins().plugins.iter().position(|p| p.name == n);
        let before = pos(&mut a, "D.esp");
        let ev = a
            .command(Command::MovePlugins {
                names: vec!["D.esp".into()],
                before: None,
            })
            .unwrap();
        assert!(matches!(ev, Event::Changed { canon: true, .. }));
        assert!(pos(&mut a, "D.esp") >= before);
        assert!(matches!(
            a.command(Command::MovePlugins {
                names: vec!["Skyrim.esm".into()],
                before: None
            }),
            Err(AppError::PluginOrder(_))
        ));
    }

    #[test]
    fn marks_are_edited_by_command_and_shown_on_rows_and_in_the_summary() {
        let mut a = app();
        let k = a.session().book().key_of("A").unwrap();
        for edit in [
            MarksEdit::AddTag { tag: "UI".into() },
            MarksEdit::AddSticker {
                sticker: "⭐".into()
            },
            MarksEdit::SetNote {
                note: "check after update".into(),
            },
        ] {
            a.command(Command::EditMarks { keys: vec![k], edit }).unwrap();
        }
        let Reply::ModDetail { detail } = a.query(Query::ModDetail { key: k }).unwrap() else {
            panic!()
        };
        let m = detail.marks.unwrap();
        assert_eq!(
            (m.tags, m.stickers, m.note.as_str()),
            (
                vec!["UI".to_string()],
                vec!["⭐".to_string()],
                "check after update"
            )
        );
        let Reply::Tags { tags, catalog } = a.query(Query::Tags).unwrap() else {
            panic!()
        };
        assert_eq!(
            (tags[0].name.as_str(), tags[0].entries, tags[0].color),
            ("UI", 1, 0x3b82f6)
        );
        assert!(catalog.find("SKSE").is_some());
        a.command(Command::Undo).unwrap();
        let Reply::Rows { rows, .. } = a
            .query(Query::Rows {
                view: order::MAIN,
                start: 0,
                count: 3,
                badges: 0,
            })
            .unwrap()
        else {
            panic!()
        };
        let m = rows.iter().find(|r| r.key == k).unwrap().marks.clone().unwrap();
        assert!(
            m.note.is_empty() && m.tags == ["UI"],
            "undo took back the note only"
        );
    }
}
