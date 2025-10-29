# rldx – Agent Notes

## TL;DR
- `rldx` is a Rust 2021 TUI application for browsing and managing local vCard directories.
- On startup it loads user configuration, normalizes the vdir layout, keeps an SQLite index in the XDG data directory, and renders a Ratatui UI.
- A Google Contacts importer is available via the CLI; no other importers or CardDAV sync are wired up yet.

## Build & Run
- Build and run with `cargo run --release` (or `cargo run` during development).
- Running the binary as-is expects a config file at `${XDG_CONFIG_HOME:-~/.config}/rldx/config.toml`. Copy or adapt `config.example.toml` before launching.
- The optional `--reindex` flag forces a full re-read of the vdir even if file hashes and mtimes have not changed.
- `cargo test` currently does nothing because there are no test targets defined.

## Configuration & Data Locations
- `src/config.rs` enforces the presence of `vdir`, drives UI key bindings/field layout, and handles the optional external copy command.
- The SQLite index lives at `${XDG_DATA_HOME:-~/.local/share}/rldx/index.db` and is created on demand.
- The vdir normalization logic writes a `.rldx_normalized` marker once it has rewritten files to single-card, vCard 4.0 `.vcf` entries.

## Key Components
- `src/main.rs`: CLI entry point. Supports `import google` and launches the TUI by default.
- `src/vdir.rs`: Normalizes vCard files, tracks sha1 + mtime for change detection, and handles atomic writes.
- `src/indexer.rs`: Builds `IndexedItem` and `IndexedProp` records from vCard data (names, emails, phones, org, etc.).
- `src/db.rs`: Manages the SQLite schema, upserts indexed data, and provides listing helpers used by the UI.
- `src/ui/`: Ratatui application (search pane, card view, detail tabs, inline editor scaffold, copy-to-clipboard helper).
- `src/import/google.rs`: Converts Google export files to vCard 4.0, ensures UUIDs/REV fields, and saves them into the configured vdir.
- `src/search.rs`: Helper for query normalization used when filtering contacts.

## Current Limitations / Open Work
- Only Google vCard exports are supported; additional importers or remote sync are not implemented.
- Inline editing UI scaffolding exists but no save/apply logic is connected yet (editing remains read-only).
- Photo fetching/rendering is still unimplemented.
- Error handling is mostly surfaced via stderr warnings; there is no telemetry or logging layer beyond that.
- Automated tests are absent; consider adding integration coverage for normalization, indexing, and the importer.

## Useful Commands
- `cargo run -- --reindex` to rebuild the index after manual vdir edits.
- `cargo run -- import --format google contacts.vcf [--book some/subdir]` to convert and place Google exports into the vdir.
- `cargo fmt` / `cargo clippy` to keep the code formatted and linted (not enforced, but both pass on current sources).
