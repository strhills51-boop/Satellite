//! Dev shell: loads an MO2 build read-only and serves the list window on localhost (ADR 0004). @sat:devserver
//!
//! Owns: the HTTP transport (`POST /command`, `POST /query`, `GET /`) and loading a build into a session for it.
//! Not here: what commands and queries mean (`@sat:app`), the page (`apps/devserver/src/index.html`).
//!
//! Sandbox: nothing is ever written to the build or the project; edits live in memory until the server stops.
//! The scan uses the same cache as `sat-cli analyze` (`<profile>\satellite\cache`), so a build `sat-cli` has
//! analyzed opens in seconds.

use std::sync::Arc;
use std::time::Instant;

use sat_app::{App, Command, Query};
use sat_core::canon::Modlist;
use sat_core::order::OrderBook;
use sat_core::plugins::{PluginCanon, PluginVersions};
use sat_core::project::{PROJECT_SCHEMA, Project};
use sat_core::resource::Registry;
use sat_core::session::{OVERWRITE, Session};
use sat_core::table::TableBuilder;
use sat_mo2::{Instance, profile};
use sat_scan::{ScanJob, ScanOptions, ScanProgress};

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
/// Opens the build: the project's canon and decorations if `sat-cli` saved one, else `modlist.txt`; scans (cached)
/// every listed mod and Overwrite; reads the plugin lists.
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
    session.set_member_keys(p.member_keys());
    session.set_decisions(p.decisions.clone());
    session.set_conflict_ignore(p.conflict_ignore.clone());
    session.set_plugin_facts(Arc::new(versions));
    // the manager's plugin lists as the last analysis saw them, and the user's hidden plugins
    session.set_plugin_canon(p.plugins.clone());
    let mut app = App::new(session, Registry::with_builtins());
    app.set_sync(p.sync.clone(), Vec::new());
    app.set_tag_catalog(p.tags.clone());
    for e in app.set_sticker_rules(&p.sticker_rules) {
        eprintln!("{e}");
    }
    Ok(app)
}

fn load(args: &[String]) -> Result<App, String> {
    if let Some(dir) = flag(args, "--project") {
        return load_offline(std::path::Path::new(dir));
    }
    let path = args
        .first()
        .filter(|a| !a.starts_with("--"))
        .ok_or("usage: sat-devserver <MO2 folder> [--profile NAME] [--port N]\n       sat-devserver --project <project.stll or its folder> [--port N]   (no build needed)")?;
    let mut inst = Instance::load(path).map_err(|e| e.to_string())?;
    if let Some(p) = flag(args, "--profile") {
        inst.profile = p.to_string();
    }
    let read = |f: &str| std::fs::read(inst.profile_dir().join(f));
    let modlist = read("modlist.txt").map_err(|e| format!("modlist.txt: {e}"))?;
    let manager: Modlist = profile::parse_modlist(&modlist).0;
    let sat_dir = inst.profile_dir().join("satellite");
    let project_file = sat_dir.join(sat_stll::FILE_NAME);
    let mut decisions = None;
    let mut member_keys = Vec::new();
    let mut records = Vec::new();
    let mut blacklist = Vec::new();
    let mut ignore = Default::default();
    let mut tags = sat_core::tags::TagCatalog::default();
    let mut sticker_rules = Vec::new();
    // the project file; the early layout is read as it is (the sandbox converts nothing)
    let opened = if project_file.exists() {
        sat_stll::Stll::open(&project_file)
            .map(|(_, p)| p)
            .map_err(|e| eprintln!("{e}"))
            .ok()
    } else {
        std::fs::read(sat_dir.join("project.v0.json"))
            .ok()
            .and_then(|b| Project::from_json(&b).ok())
    };
    let book = match opened {
        Some(p) if p.schema == PROJECT_SCHEMA => {
            eprintln!(
                "canon: project {} ({} settled chains)",
                project_file.display(),
                p.decisions.len()
            );
            decisions = Some(p.decisions.clone());
            member_keys = p.member_keys().collect();
            records = p.sync.clone();
            blacklist = p.plugins.blacklist.clone();
            ignore = p.conflict_ignore.clone();
            tags = p.tags.clone();
            sticker_rules = p.sticker_rules.clone();
            p.book().0
        }
        _ => {
            eprintln!("canon: modlist.txt (no Satellite project yet)");
            OrderBook::from_modlist(&manager)
        }
    };
    let list = book.to_modlist();
    let mut jobs: Vec<ScanJob> = list
        .entries
        .iter()
        .filter(|e| e.is_mod())
        .map(|e| ScanJob {
            name: e.name.clone(),
            path: inst.mod_dir(&e.name),
        })
        .collect();
    jobs.push(ScanJob {
        name: OVERWRITE.into(),
        path: inst.overwrite_dir.clone(),
    });
    let opts = ScanOptions {
        cache_dir: Some(inst.profile_dir().join("satellite").join("cache")),
        ..Default::default()
    };
    let t = Instant::now();
    let progress = ScanProgress::default();
    let mut builder = TableBuilder::new().with_tag_rules(builtin_rules());
    let mut versions = PluginVersions::default();
    sat_scan::scan_batched(&jobs, &opts, &progress, 256, |_, o| {
        let id = builder.add(&o.facts);
        versions.add(id, &o.facts);
    });
    let (_, total, cached, files) = progress.snapshot();
    eprintln!(
        "scan: {total} mods ({cached} from cache), {files} files in {:.1} s",
        t.elapsed().as_secs_f64()
    );
    let table = Arc::new(builder.finish());
    let mut session = Session::new(book, table);
    session.set_member_keys(member_keys);
    if let Some(d) = decisions {
        session.set_decisions(d);
    }
    session.set_conflict_ignore(ignore);
    session.set_plugin_facts(Arc::new(versions));
    session.set_plugin_canon(PluginCanon {
        load_order: profile::parse_loadorder(&read("loadorder.txt").unwrap_or_default()),
        checked: profile::parse_plugins(&read("plugins.txt").unwrap_or_default())
            .into_iter()
            .map(|l| (l.name, l.enabled))
            .collect(),
        blacklist,
    });
    let t = Instant::now();
    let totals = session.overwrite().totals();
    eprintln!(
        "overwrite: {} conflict paths, {} chains in {:.0} ms",
        totals.conflict_paths,
        totals.chains,
        t.elapsed().as_secs_f64() * 1000.0
    );
    let mut app = App::new(session, Registry::with_builtins());
    // the records of the last `sat-cli analyze` and the folders now: decisions and the plan, never a commit here
    let disk: Vec<String> = std::fs::read_dir(&inst.mods_dir)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    eprintln!("sync: {} record(s) from the project", records.len());
    app.set_sync(records, disk);
    app.set_tag_catalog(tags);
    for e in app.set_sticker_rules(&sticker_rules) {
        eprintln!("{e}");
    }
    Ok(app)
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
    let mut app = match load(&args) {
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
    eprintln!("open http://127.0.0.1:{port}/  (sandbox: nothing is saved; Ctrl+C to stop)");
    for mut req in server.incoming_requests() {
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
    }
}
