//! Dev shell: loads an MO2 build read-only and serves the list window on localhost (ADR 0004). @sat:devserver
//!
//! Owns: the HTTP transport (`POST /command`, `POST /query`, `GET /`) and loading a build into a session for it.
//! Not here: what commands and queries mean (`@sat:app`), the page (`apps/devserver/src/index.html`).
//!
//! Sandbox by default: nothing is ever written to the build or the project; edits live in memory until the server
//! stops. With `--write` the project is saved when the window asks (ADR 0019 §5), and a full run writes the facts
//! it saw into it, like `sat-cli analyze`.
//! The scan uses the same cache as `sat-cli analyze` (`<profile>\satellite\cache`), so a build `sat-cli` has
//! analyzed opens in seconds.

use std::sync::Arc;
use std::time::Instant;

use sat_app::{App, Command, Query};
use sat_core::plugins::PluginVersions;
use sat_core::resource::Registry;
use sat_core::session::{OVERWRITE, Session};
use sat_core::table::TableBuilder;
use sat_mo2::Instance;
use sat_scan::ScanProgress;

mod home;

const PAGE: &str = include_str!("index.html");

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
}

/// The built-in tag rules (the monolith's detectors): found tags show in the mod sheet and in `Find`.
fn builtin_rules() -> Arc<sat_core::tag_rules::Compiled> {
    Arc::new(sat_core::tag_rules::Compiled::builtin())
}

// @sat:devserver.load
/// Opens the build: the project's canon and decorations if `sat-cli` saved one, else `modlist.txt`; runs over it
/// like `sat-cli analyze` (`@sat:home.run`: every folder, Overwrite, sync records, sources, plugin lists) without
/// writing anything.
// @sat:devserver.offline
/// Opens a project file (`project.stll`, or the folder holding it) with no build at all: the order, decoration,
/// conflicts, plugins and metrics come from the project (ADR 0007 — the project is self-sufficient).
fn load_offline(at: &std::path::Path) -> Result<App, String> {
    let file = if at.is_dir() {
        at.join(sat_stll::FILE_NAME)
    } else {
        at.to_path_buf()
    };
    if !file.exists() && file.with_file_name("project.v0.json").exists() {
        return Err(format!(
            "{}: an early project layout — run `sat-cli analyze` once to convert it (the sandbox writes nothing)",
            file.with_file_name("project.v0.json").display()
        ));
    }
    let (mut store, p) = sat_stll::Stll::open(&file).map_err(|e| e.to_string())?;
    let dir = file.as_path(); // for the log line
    let t = Instant::now();
    let mut builder = TableBuilder::new().with_tag_rules(builtin_rules());
    let mut versions = PluginVersions::default();
    let (mut found, mut absent) = (0, 0);
    let names = p
        .canon
        .entries
        .iter()
        .filter(|e| e.is_mod())
        .map(|e| (e.name.clone(), p.entities.id_of(&e.name).cloned()))
        .chain(std::iter::once((
            OVERWRITE.to_string(),
            Some(sat_core::entity::EntityId("overwrite".into())),
        )));
    for (name, id) in names {
        let facts = match id.and_then(|id| store.facts(&id.0)) {
            Some((_, f)) => {
                found += 1;
                f
            }
            None => {
                absent += 1;
                sat_core::facts::ModFacts::missing(name)
            }
        };
        let mid = builder.add(&facts);
        versions.add(mid, &facts);
    }
    eprintln!(
        "offline: project {} — facts of {found} mods ({absent} without facts) in {:.1} s",
        dir.display(),
        t.elapsed().as_secs_f64()
    );
    let mut session = Session::new(p.book().0, Arc::new(builder.finish()));
    session.set_plugin_facts(Arc::new(versions));
    let mut app = App::new(session, Registry::with_builtins());
    // the manager's plugin lists as the last analysis saw them, and the user's hidden plugins
    for e in app.load_project(&p, Vec::new()) {
        eprintln!("{e}");
    }
    Ok(app)
}

fn load(args: &[String], after_commit: bool) -> Result<(App, Option<home::Home>), String> {
    if let Some(dir) = flag(args, "--project") {
        return Ok((load_offline(std::path::Path::new(dir))?, None));
    }
    let path = args
        .first()
        .filter(|a| !a.starts_with("--"))
        .ok_or("usage: sat-devserver <MO2 folder> [--profile NAME] [--game DIR] [--port N] [--write]\n       sat-devserver --project <project.stll or its folder> [--port N]   (no build needed)")?;
    let mut inst = Instance::load(path).map_err(|e| e.to_string())?;
    // the game pack (`@sat:game`): what the core knows about this game
    sat_core::game::select(&inst.game_name);
    if let Some(p) = flag(args, "--profile") {
        inst.profile = p.to_string();
    }
    if let Some(g) = flag(args, "--game") {
        inst.game_path = g.into();
    }
    if inst.game_path.join("Data").is_dir() {
        eprintln!("game: {}", inst.game_path.display());
    } else {
        eprintln!(
            "game: NOT FOUND ({:?}) - base masters, Creation Club plugins and the game version are unknown; give --game <folder with Data>",
            inst.game_path.display().to_string()
        );
    }
    // the same run as `sat-cli analyze` (`@sat:home.run`); opening writes nothing, even with `--write` — no facts,
    // no project, the early layout read as it is. After a commit (with `--write`) the opening run is the commit's
    // check and saves like `analyze` does after `commit`: the facts of adopted mods, their signatures.
    let write = args.iter().any(|a| a == "--write");
    let save = write && after_commit;
    let path = sat_home::project::default_path(&inst);
    let mut stamp = sat_home::project::stamp_of(&path);
    let manager = sat_home::project::manager_list(&inst)?;
    let opened = sat_home::project::open_for_run(
        &path,
        manager,
        sat_home::project::OpenOptions {
            reimport: false,
            read_only: true,
        },
    )?;
    // what is on disk now: unsaved changes are measured against it (none yet — the window must save first)
    let mut on_disk = (opened.origin == sat_home::project::Origin::Stored).then(|| opened.project.clone());
    eprintln!("{}", opened.origin_line());
    let sat_home::project::Opened {
        mut stll,
        mut project,
        ..
    } = opened;
    let inv = sat_home::run::inventory(&inst, &project.canon);
    let jobs = sat_home::run::jobs(&inst, &project.canon, &inv.unlisted, true);
    let mut o = sat_home::run::RunOptions::for_build(&inst);
    o.save_facts = save;
    o.tag_rules = Some(builtin_rules());
    let t = Instant::now();
    let progress = ScanProgress::default();
    let (scanned, pending) =
        sat_home::run::scan(&inst, &mut stll, &mut project, &jobs, &o, &progress, &mut |_| {})?;
    let (_, total, cached, files) = progress.snapshot();
    eprintln!(
        "scan: {total} mods ({cached} from cache, {} not listed), {files} files in {:.1} s",
        inv.unlisted.len(),
        t.elapsed().as_secs_f64()
    );
    let mut session = Session::new(project.book().0, Arc::new(scanned.table));
    session.set_plugin_facts(Arc::new(scanned.plugin_versions));
    if scanned.plugins.pending {
        eprintln!("plugins: the project has edits of its own");
    }
    session.set_plugin_canon(project.plugins.clone());
    let t = Instant::now();
    let totals = session.overwrite().totals();
    eprintln!(
        "overwrite: {} conflict paths, {} chains in {:.0} ms",
        totals.conflict_paths,
        totals.chains,
        t.elapsed().as_secs_f64() * 1000.0
    );
    let published = pending.publish(
        &project,
        sat_home::run::Totals {
            chains: totals.chains,
            contexts: totals.contexts,
            conflict_paths: totals.conflict_paths,
        },
        save,
    )?;
    for v in &published.seals {
        eprintln!("seal: {v}");
    }
    if published.saved.is_some() {
        eprintln!("project and facts saved → {}", path.display());
        on_disk = Some(project.clone());
        stamp = sat_home::project::stamp_of(&path);
    }
    let mut app = App::new(session, Registry::with_builtins());
    // the records of this run against the folders now: decisions and the plan, never a commit here
    for e in app.load_project(&project, sat_home::subfolders(&inst.mods_dir)) {
        eprintln!("{e}");
    }
    let r = &scanned.sync;
    eprintln!(
        "sync: {} record(s), {} pending",
        r.records.len(),
        r.count(sat_core::sync::records::Status::Pending)
    );
    // conflict domains described by data (`@sat:conflict.spec`): the game pack's and the user's own, reading the
    // files they rank by from the mod folders
    {
        let (mods_dir, overwrite_dir) = (inst.mods_dir.clone(), inst.overwrite_dir.clone());
        app.set_file_reader(std::sync::Arc::new(move |m: &str, rel: &str| {
            let dir = if m == sat_core::session::OVERWRITE {
                overwrite_dir.clone()
            } else {
                mods_dir.join(m)
            };
            std::fs::read(dir.join(rel)).ok()
        }));
        // the file explorer reads copies of files, loose or packed (`@sat:scan.fetch`)
        // and the folders outside the build: the game, the profile, the instance, the game's documents
        let folders = sat_scan::fetch::Folders {
            mods: inst.mods_dir.clone(),
            overwrite: inst.overwrite_dir.clone(),
        };
        let mut places = vec![
            ("game", "Игра", inst.game_path.clone()),
            ("profile", "Профиль", inst.profile_dir()),
            ("instance", "MO2", inst.base_dir.clone()),
        ];
        if let Some(dir) = inst.ini_path.parent().filter(|d| *d != inst.base_dir) {
            places.push(("mo2ini", "MO2 (ini экземпляра)", dir.to_path_buf()));
        }
        if let Some(d) = inst.documents_dir() {
            places.push(("documents", "Документы игры", d));
        }
        let disk = sat_scan::fetch::Disk::new(places);
        for p in disk.places() {
            eprintln!("explorer: {} = {}", p.label, p.root);
        }
        app.set_file_source(std::sync::Arc::new(Files { folders, disk }));
        let rt = sat_scan::game_runtime(&inst.game_path);
        if let Some(v) = rt.version {
            eprintln!("game: {}.{}.{}.{}", v.0, v.1, v.2, v.3);
        }
        app.set_game_runtime(rt);
        let user = inst.profile_dir().join("satellite").join("conflict-domains.json");
        if let Ok(text) = std::fs::read_to_string(&user) {
            match serde_json::from_str::<Vec<sat_core::conflict_spec::DomainSpec>>(&text) {
                Ok(specs) => {
                    eprintln!("conflict domains: {} from {}", specs.len(), user.display());
                    app.set_user_domains(specs);
                }
                Err(e) => eprintln!("{}: {e}", user.display()),
            }
        }
    }
    start_readers(&inst, &mut app);
    app.enable_passes();
    app.set_plan_source(Arc::new(home::HomePlan {
        layout: sat_mo2::commit::Layout::of(&inst),
        inst: inst.clone(),
    }));
    if let Some(p) = on_disk {
        app.project_saved(p);
    }
    if write {
        app.enable_saving(path.display().to_string());
        app.enable_commits();
        eprintln!(
            "write: the project is saved on request (Ctrl+S in the window) → {}",
            path.display()
        );
    }
    let home = home::Home::new(inst, path, write, stamp);
    Ok((app, Some(home)))
}

// @sat:devserver.readers
/// Starts what the window reads off the request loop after facts arrive: content hashes of contested copies
/// (`@sat:identical`, from the cache `analyze` keeps) and the record layer (`@sat:records`) with its fields. Until
/// they arrive copies are "not compared" and the record layer is pending.
pub(crate) fn start_readers(inst: &Instance, app: &mut App) {
    // content of contested copies (`@sat:identical`), from the hash cache `analyze` keeps; what is missing is read
    // off the request loop (a first run reads a lot) — until then copies are "not compared"
    {
        let table = app.session().table().clone();
        let (mods_dir, overwrite_dir) = (inst.mods_dir.clone(), inst.overwrite_dir.clone());
        let cache = inst.profile_dir().join("satellite").join("content-hashes.txt");
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let t = std::time::Instant::now();
            let (hashes, read) = sat_scan::content::hashes_for(
                &table,
                |m| {
                    if m == sat_core::session::OVERWRITE {
                        overwrite_dir.clone()
                    } else {
                        mods_dir.join(m)
                    }
                },
                &cache,
                0,
            );
            eprintln!(
                "content hashes: {read} file(s) and archive(s) read in {:.1} s",
                t.elapsed().as_secs_f64()
            );
            let _ = tx.send(hashes);
        });
        HASHES.with(|h| *h.borrow_mut() = Some(rx));
    }
    // @sat:devserver.records
    // the record layer (`@sat:records`): plugins are read off the request loop, the window shows it when ready
    let loaded = sat_scan::records::loaded_plugins(
        &app.session().plugins(),
        app.session().table(),
        |m| {
            if m == sat_core::session::OVERWRITE {
                inst.overwrite_dir.clone()
            } else {
                inst.mods_dir.join(m)
            }
        },
        &inst.game_path.join("Data"),
    );
    let cache = inst.profile_dir().join("satellite").join("records");
    let (tx, rx) = std::sync::mpsc::channel();
    let (ftx, frx) = std::sync::mpsc::channel();
    app.records_pending();
    std::thread::spawn(move || {
        let t = std::time::Instant::now();
        let (ix, unread, read) = sat_scan::records::index(&loaded, &cache);
        eprintln!(
            "records: {} in {} plugin(s), {read} read, {:.1} s",
            ix.records(),
            ix.plugins.len(),
            t.elapsed().as_secs_f64()
        );
        let report = sat_app::records::RecordReport::from_index(ix, unread);
        let ix = report.index.clone();
        let _ = tx.send(report);
        // then the fields of the conflict records (`@sat:scan.records.fields`)
        let t = std::time::Instant::now();
        let f = sat_scan::records::fields(&loaded, &ix);
        eprintln!(
            "record fields: {} copies of conflict records in {:.1} s",
            f.0.len(),
            t.elapsed().as_secs_f64()
        );
        let _ = ftx.send(f);
    });
    RECORDS.with(|r| *r.borrow_mut() = Some(rx));
    FIELDS.with(|r| *r.borrow_mut() = Some(frx));
}

thread_local! {
    /// Content hashes on their way from the reading thread.
    static HASHES: std::cell::RefCell<Option<std::sync::mpsc::Receiver<sat_core::identical::ContentHashes>>> =
        const { std::cell::RefCell::new(None) };
    /// The record report on its way from the reading thread.
    static RECORDS: std::cell::RefCell<Option<std::sync::mpsc::Receiver<sat_app::records::RecordReport>>> =
        const { std::cell::RefCell::new(None) };
    /// Fields of the conflict records, after the records.
    static FIELDS: std::cell::RefCell<Option<std::sync::mpsc::Receiver<sat_core::conflict_fields::FieldFacts>>> =
        const { std::cell::RefCell::new(None) };
}

fn json(body: String, status: u16) -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
    tiny_http::Response::from_string(body)
        .with_status_code(status)
        .with_header(
            "Content-Type: application/json; charset=utf-8"
                .parse::<tiny_http::Header>()
                .expect("header"),
        )
}

// @sat:devserver.http
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (mut app, mut home) = match load(&args, false) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(2);
        }
    };
    let port: u16 = flag(&args, "--port").and_then(|p| p.parse().ok()).unwrap_or(7878);
    // localhost only: the API changes state and has no authentication
    let server = tiny_http::Server::http(("127.0.0.1", port)).unwrap_or_else(|e| {
        eprintln!("error: port {port}: {e}");
        std::process::exit(2);
    });
    eprintln!(
        "open http://127.0.0.1:{port}/  ({}; Ctrl+C to stop)",
        if args.iter().any(|a| a == "--write") {
            "--write: the project is saved when asked"
        } else {
            "sandbox: nothing is saved"
        }
    );
    for mut req in server.incoming_requests() {
        reopen(&args, &mut app, &mut home);
        RECORDS.with(|r| {
            if let Some(report) = r.borrow().as_ref().and_then(|rx| rx.try_recv().ok()) {
                app.set_records(report);
            }
        });
        FIELDS.with(|r| {
            if let Some(f) = r.borrow().as_ref().and_then(|rx| rx.try_recv().ok()) {
                app.set_record_fields(f);
            }
        });
        HASHES.with(|h| {
            if let Some(hashes) = h.borrow().as_ref().and_then(|rx| rx.try_recv().ok()) {
                app.set_content_hashes(hashes);
            }
        });
        let mut body = String::new();
        let _ = req.as_reader().read_to_string(&mut body);
        let url = req.url().to_string();
        let reply = match (req.method(), url.as_str()) {
            (tiny_http::Method::Get, "/") => {
                let page = tiny_http::Response::from_string(PAGE).with_header(
                    "Content-Type: text/html; charset=utf-8"
                        .parse::<tiny_http::Header>()
                        .expect("header"),
                );
                let _ = req.respond(page);
                continue;
            }
            (tiny_http::Method::Post, "/command") => match serde_json::from_str::<Command>(&body) {
                Ok(c) => match app.command(c) {
                    Ok(ev) => json(serde_json::to_string(&ev).expect("event"), 200),
                    Err(e) => json(serde_json::json!({ "error": e.to_string() }).to_string(), 422),
                },
                Err(e) => json(serde_json::json!({ "error": e.to_string() }).to_string(), 400),
            },
            (tiny_http::Method::Post, "/query") => match serde_json::from_str::<Query>(&body) {
                Ok(q) => match app.query(q) {
                    Ok(r) => json(serde_json::to_string(&r).expect("reply"), 200),
                    Err(e) => json(serde_json::json!({ "error": e.to_string() }).to_string(), 422),
                },
                Err(e) => json(serde_json::json!({ "error": e.to_string() }).to_string(), 400),
            },
            _ => json(r#"{"error":"not found"}"#.into(), 404),
        };
        let _ = req.respond(reply);
        // a run asked by this request starts now, not at the next one
        reopen(&args, &mut app, &mut home);
    }
}

// @sat:devserver.reopen
/// Ticks the build's runs and commits; after a commit wrote the build, opens it again — the run of the reopened
/// build is the commit's check (ADR 0019 §4: no mismatches expected), and the window reloads.
fn reopen(args: &[String], app: &mut App, home: &mut Option<home::Home>) {
    let Some(r) = home.as_mut().and_then(|h| h.tick(app)) else {
        return;
    };
    eprintln!("commit: done — opening the build again");
    match load(args, true) {
        Ok((mut a, h)) => {
            let mut r = r;
            r.records_after = Some(a.sync_records().len());
            a.commit_finished(r);
            *app = a;
            *home = h;
        }
        Err(e) => {
            let mut r = r;
            r.error = Some(format!("committed, but opening the build again failed: {e}"));
            app.commit_finished(r);
        }
    }
}

/// The explorer's disk (`@sat:scan.fetch`): copies of the build's files and the places outside it.
struct Files {
    folders: sat_scan::fetch::Folders,
    disk: sat_scan::fetch::Disk,
}

impl sat_app::FileSource for Files {
    fn copy(&self, m: &str, rel: &str, archive: Option<&str>) -> Option<sat_formats::preview::Preview> {
        sat_scan::fetch::preview(&self.folders, m, rel, archive)
    }
    fn places(&self) -> Vec<sat_formats::preview::Place> {
        self.disk.places()
    }
    fn list(&self, place: &str, dir: &str) -> Option<Vec<sat_formats::preview::DiskEntry>> {
        self.disk.list(place, dir)
    }
    fn read(&self, place: &str, path: &str) -> Option<sat_formats::preview::Preview> {
        self.disk.read(place, path)
    }
    fn game(&self, path: &str) -> Option<sat_formats::preview::Preview> {
        self.disk.game(path)
    }
}
