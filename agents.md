# rldx - Agent Notes

## Overview

rldx is a Rust TUI contact manager for vCard 4.0 directories. The vdir (filesystem) is canonical; the SQLite database is a rebuildable index/cache.

## Build & Run

```bash
cargo run --release                # Normal run
cargo run -- --reindex             # Force full reindex
cargo run -- import --format google contacts.vcf  # Import Google export
```

Requires config at `~/.config/rldx/config.toml` (see `config.example.toml`). The only required setting is `vdir = "/path/to/contacts"`.

## Architecture

```
rldx/src/
  main.rs       # CLI entry, startup flow, reindexing
  config.rs     # TOML config loading, validation, key binding system
  db.rs         # SQLite schema (items + props tables), queries
  indexer.rs    # Builds IndexedItem/IndexedProp from vCard data
  search.rs     # Query normalization helpers
  vcard_io.rs   # vCard 4.0 parsing/writing via vcard4 crate
  vdir.rs       # Filesystem ops, normalization, atomic writes
  ui/
    app.rs      # App state, event loop, key handling (~2300 lines)
    draw.rs     # Ratatui rendering (~850 lines)
    edit.rs     # Inline editor widget
    panes.rs    # Detail tab definitions
  import/
    google.rs   # Google Contacts vCard 3.0 -> 4.0 converter
```

## Key Data Flow

1. **Startup**: Load config -> validate key bindings -> normalize vdir (first run) -> open SQLite -> reindex changed files -> launch TUI
2. **Edit**: Parse vCard -> modify in memory -> atomic file write -> DB upsert
3. **Merge**: Mark contacts -> merge vCards inductively -> write new card -> delete old cards -> reindex

## Implemented Features

- Four-pane UI (search, card, image, detail tabs)
- Inline field editing with save to disk
- Contact merging (mark multiple, press 'm')
- Multi-value modals for email/phone fields
- Phone number normalization (E.164 via rlibphonenumber)
- Photo/logo display (kitty graphics protocol)
- Google Contacts import
- Fully configurable keybindings with context-aware binding system

## Key Binding System

Key bindings are organized by context to avoid collisions. Each action can have multiple bindings.

**Contexts:**
- `keys.global` - quit, search, help (work everywhere)
- `keys.search_input` - cancel, confirm (when typing search)
- `keys.search_results` - navigation, mark, merge (in result list)
- `keys.navigation` - field navigation, edit, copy (in card/detail panes)
- `keys.modal` - dialog actions (multivalue, confirm, alias modals)
- `keys.editor` - inline editing (cancel, confirm)

See `config.example.toml` for full documentation of available keys and defaults.

## Not Implemented

- Photo fetching from URLs (scaffolded but not wired)
- CardDAV sync (libdav is a dependency but not connected)
- Help modal (F1/? bound but not implemented)
- Automated tests

## Making Changes

**Add a new vCard field to display:**
1. Extract in `indexer.rs` (add to `collect_*_props` functions)
2. Build UI field in `ui/app.rs` (`build_*_fields` functions)

**Add a new keybinding:**
1. Add field to appropriate context struct in `config.rs` (e.g., `NavigationKeys`)
2. Add default binding in the struct's `Default` impl
3. Add to deserialization struct and conversion
4. Add to collision validation in `validate_key_bindings()`
5. Handle in the appropriate handler in `ui/app.rs`

**Add a new import format:**
1. Create module in `import/`
2. Add variant to `ImportFormat` enum in `main.rs`
3. Call from `handle_import()`

## Code Style

- Explicit, straightforward code over clever abstractions
- `anyhow::Result` for error handling
- Heavy use of `Option` and pattern matching
- UI rendering separated from state management
- Atomic writes: temp file -> fsync -> rename -> dir fsync
- Key handling uses `key_matches_any()` for Vec<String> bindings
