# Satellite — New Generation of ModlistTool.mo2 | Monolith

Mod management from orbit: observe, understand, declare, apply. Satellite never hosts mods.

- **Direction and rules:** [`PHILOSOPHY.md`](PHILOSOPHY.md) — read first.
- **Decisions:** [`docs/adr/`](docs/adr) · **Work in progress:** [`docs/distances/`](docs/distances)
- **UI parity reference:** [`docs/UI-PARITY.md`](docs/UI-PARITY.md), screenshots in [`docs/parity/`](docs/parity)

## Layout

| Path | What |
|---|---|
| `crates/sat-formats` | TES4 / BSA / BA2 / DDS parsers, no filesystem access (WASM-ready) |
| `crates/sat-core` | domain core: ids, path interner, provider table, overwrite projection, canon — no I/O |
| `crates/sat-mo2` | Mod Organizer 2 adapter: instance, modlist/plugins/loadorder, meta.ini |
| `crates/sat-scan` | parallel scanner with a signature-keyed scan cache |
| `tools/sat-cli` | developer CLI: `analyze <instance>`, `bench-synth` |
| `prototypes/go-spike` | frozen v0.1 prototype, used as a reference for result parity |

## Develop

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo run --release -p sat-cli -- analyze "D:\MO2" --extra "E:\mods2" --report report.json
cargo run --release -p sat-cli -- bench-synth --mods 5000 --files 1100
```

License: MPL-2.0 (proposed, see ADR 0003).
