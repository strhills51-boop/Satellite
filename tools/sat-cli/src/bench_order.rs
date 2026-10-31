//! `sat-cli bench-order` — the load order engine (ADR 0008) on a synthetic list of N entries. @sat:cli.bench-order
//!
//! Owns: the load order engine benchmark.

use std::time::Instant;

use sat_core::canon::{Entry, EntryKind, Modlist};
use sat_core::order::{Change, Filter, MAIN, Nav, OrderBook, Select, Shift};

fn arg(args: &[String], name: &str, default: usize) -> usize {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn patches(c: &Option<Change>) -> String {
    match c {
        Some(c) => format!("{} patches, rows {}", c.main().patches.len(), c.main().len),
        None => "no change".into(),
    }
}

pub fn run(args: &[String]) -> Result<(), String> {
    let n = arg(args, "--entries", 1_000_000);
    let block = arg(args, "--block", 50).max(2);
    let mut rng = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = move || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng
    };
    let entries: Vec<Entry> = (0..n)
        .map(|i| {
            let sep = i % block == 0;
            Entry {
                name: if sep {
                    format!("Block {i}_separator")
                } else {
                    format!("Mod {i:07}")
                },
                kind: if sep { EntryKind::Separator } else { EntryKind::Mod },
                enabled: !sep && i % 3 != 0,
            }
        })
        .collect();
    let list = Modlist::new(entries);
    let rss = || crate::rss_mib().unwrap_or(0);

    let t = Instant::now();
    let mut b = OrderBook::from_modlist(&list);
    println!(
        "build             {:>9.1} ms   {n} entries, {} rows, rss {} MiB",
        ms(t),
        b.row_count(MAIN),
        rss()
    );

    let t = Instant::now();
    let mut names = 0usize;
    for _ in 0..1000 {
        let start = (next() as usize) % b.row_count(MAIN);
        names += b
            .rows(MAIN, start, 60)
            .iter()
            .map(|r| r.name.len())
            .sum::<usize>();
    }
    println!(
        "window read       {:>9.3} ms   60 rows at a random offset (avg of 1000; {names} bytes)",
        ms(t) / 1000.0
    );

    let t = Instant::now();
    for _ in 0..1000 {
        let k = b.key_at(1 + (next() as usize) % (n - 1));
        let target = b.key_at((next() as usize) % n);
        b.move_keys(&[k], Some(target)).map_err(|e| e.to_string())?;
    }
    println!(
        "move one          {:>9.3} ms   drag one entry anywhere (avg of 1000)",
        ms(t) / 1000.0
    );

    let scattered: Vec<u32> = (0..10_000)
        .map(|_| b.key_at((next() as usize) % n))
        .filter(|&k| b.kind(k) == EntryKind::Mod)
        .collect();
    let target = b.key_at(n / 2);
    let t = Instant::now();
    let c = b.move_keys(&scattered, Some(target)).map_err(|e| e.to_string())?;
    println!("move 10k scattered{:>9.1} ms   {}", ms(t), patches(&c));

    let sep = (1..=2 * block)
        .map(|d| b.key_at(b.len() - d))
        .find(|&k| b.kind(k) == EntryKind::Separator)
        .ok_or("no separator")?;
    let t = Instant::now();
    let c = b
        .move_keys(&[sep], Some(b.key_at(0)))
        .map_err(|e| e.to_string())?;
    println!("move block to top {:>9.3} ms   {}", ms(t), patches(&c));

    let toggles: Vec<u32> = (0..100_000).map(|_| b.key_at((next() as usize) % n)).collect();
    let t = Instant::now();
    let c = b.set_enabled(&toggles, None).map_err(|e| e.to_string())?;
    println!("toggle 100k       {:>9.1} ms   {}", ms(t), patches(&c));

    let t = Instant::now();
    let mut seps = Vec::new();
    for pos in (0..b.len()).step_by(block * 7) {
        if let Some(k) = (pos..b.len())
            .map(|i| b.key_at(i))
            .find(|&k| b.kind(k) == EntryKind::Separator)
        {
            seps.push(k);
        }
    }
    let mut collapsed = 0;
    for &k in &seps {
        if b.set_collapsed(MAIN, k, true)
            .map_err(|e| e.to_string())?
            .is_some()
        {
            collapsed += 1;
        }
    }
    println!(
        "collapse          {:>9.3} ms   per separator ({collapsed} collapsed), rows now {}",
        ms(t) / collapsed.max(1) as f64,
        b.row_count(MAIN)
    );

    let t = Instant::now();
    let c = b
        .set_filter(
            MAIN,
            Filter {
                text: "mod 05".into(),
                ..Filter::default()
            },
        )
        .map_err(|e| e.to_string())?;
    println!("filter            {:>9.1} ms   rows {}", ms(t), c.main().len);
    let t = Instant::now();
    let c = b.set_filter(MAIN, Filter::default()).map_err(|e| e.to_string())?;
    println!("filter cleared    {:>9.1} ms   rows {}", ms(t), c.main().len);

    let p = n / 3 + 1;
    let members: Vec<u32> = (p..p + 5).map(|i| b.key_at(i)).collect();
    let t = Instant::now();
    match b.create_group("Pack", &members) {
        Ok((_, c)) => println!("group of 5        {:>9.3} ms   {}", ms(t), patches(&Some(c))),
        Err(e) => println!("group of 5        skipped: {e}"),
    }

    // keyboard cursor
    let t = Instant::now();
    b.navigate(MAIN, Nav::Row(500_000), Select::Replace)
        .map_err(|e| e.to_string())?;
    for _ in 0..1000 {
        b.navigate(MAIN, Nav::Down, Select::Replace)
            .map_err(|e| e.to_string())?;
    }
    println!(
        "cursor ↓          {:>9.3} ms   per key press (avg of 1000)",
        ms(t) / 1000.0
    );
    let t = Instant::now();
    let c = b
        .navigate(MAIN, Nav::PageDown(100_000), Select::Extend)
        .map_err(|e| e.to_string())?;
    println!(
        "Shift+PgDn 100k   {:>9.1} ms   {} patches, {} selected",
        ms(t),
        c.main().patches.len(),
        b.selection_len(MAIN)
    );
    let t = Instant::now();
    let c = b.shift_selection(MAIN, Shift::Down).map_err(|e| e.to_string())?;
    println!("Alt+↓ 100k rows   {:>9.1} ms   {}", ms(t), patches(&c));
    let t = Instant::now();
    let c = b.select_all(MAIN).map_err(|e| e.to_string())?;
    println!(
        "select all        {:>9.1} ms   {} patches",
        ms(t),
        c.main().patches.len()
    );
    let t = Instant::now();
    b.clear_selection(MAIN).map_err(|e| e.to_string())?;
    b.navigate(MAIN, Nav::Home, Select::Replace)
        .map_err(|e| e.to_string())?;
    for _ in 0..1000 {
        b.shift_selection(MAIN, Shift::Down).map_err(|e| e.to_string())?;
    }
    println!(
        "Alt+↓ one entry   {:>9.3} ms   per key press (avg of 1000)",
        ms(t) / 1000.0
    );

    // a second window over the same canon: every canon operation patches both views
    let w = b.open_view().map_err(|e| e.to_string())?;
    b.set_filter(
        w,
        Filter {
            text: "mod 00042".into(),
            ..Filter::default()
        },
    )
    .map_err(|e| e.to_string())?;
    let t = Instant::now();
    for _ in 0..1000 {
        let k = b.key_at(1 + (next() as usize) % (n - 1));
        let target = b.key_at((next() as usize) % n);
        b.move_keys(&[k], Some(target)).map_err(|e| e.to_string())?;
    }
    println!(
        "move one, 2 views {:>9.3} ms   main {} rows, second window {} rows (avg of 1000)",
        ms(t) / 1000.0,
        b.row_count(MAIN),
        b.row_count(w)
    );

    let t = Instant::now();
    let mut undone = 0;
    while b.undo().is_some() {
        undone += 1;
    }
    println!("undo everything   {:>9.1} ms   {undone} operations", ms(t));
    if b.to_modlist() != list {
        return Err("undo did not restore the canon".into());
    }
    println!("canon restored: yes; rss {} MiB", rss());
    Ok(())
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}
