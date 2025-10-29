# rldx — Agent Build Spec (minimal, straightforward)

rldx is a terminal PIM viewer/editor for vCard 4.0 contact collections stored as a **vdir** (one `.vcf` per contact). It uses a small **SQLite** index for fast listing/search, renders images via the **kitty** graphics protocol, and offers an optional **CardDAV** path via **libdav**. Keep vdir canonical; the DB is a rebuildable cache. Do not implement a vCard parser yourself—use an existing Rust crate that can **read and write vCard 4.0**.

---

## 1) Project definition

**Goals for v0**:

* Read/write vCard **4.0** only.
* Normalize a vdir to “one contact per file; file named by short UUID”.
* SQLite index shaped for a fluid schema.
* Four-pane ratatui UI: Search, Main Card, Image, Tabs.
* Inline edit of existing fields; no “add field” modals.
* Explicit key to fetch `PHOTO`/`LOGO` from URL and embed as `data:`.
* Atomic writes for both `.vcf` and DB.
* No merge, no hooks, no search DSL, no export.

**Binary name**: `rldx`.

---

## 2) Tech and dependencies

* **UI**: `ratatui` + `crossterm`.
* **Config**: `serde` + `toml` + `directories`.
* **DB**: `rusqlite` (`bundled` feature for portability).
* **Hash/time**: `sha1`, `time`.
* **IDs**: `uuid` (v4).
* **HTTP + image**: `reqwest`, `image`, `base64`.
* **vCard**: pick a maintained crate that supports **vCard 4.0 round-trip** (parse → modify → render). Do **not** write your own parser. Acceptable choices are crates that:

  1. parse property name, params, and value,
  2. preserve unknown properties/params, and
  3. emit valid 4.0 on write (folding/escaping).
     If the chosen crate lacks a writer, use another that does; round-trip fidelity is mandatory.
* **CardDAV**: `libdav` (or an equivalent CardDAV/WebDAV client crate). For v0, implement a **sync-once** function invoked by a CLI flag; do not run background sync.

Pin versions in `Cargo.toml`. No nonstandard toolchains.

---

## 3) Repository layout

```
rldx/
  Cargo.toml
  src/
    main.rs              # CLI + app start
    config.rs            # load/validate config
    vcard_io.rs          # read/write vCard using the chosen crate
    vdir.rs              # scan, normalize, atomic file writes
    db.rs                # SQLite open/DDL/CRUD and simple queries
    search.rs            # substring filtering (no DSL)
    photo.rs             # fetch/decode/cache/embed images
    ui/
      app.rs             # app state + event loop
      draw.rs            # layout + rendering
      panes.rs           # search, main card, image, tabs
      edit.rs            # inline edit widget
    dav.rs               # optional: CardDAV sync-once (feature "dav")
  tests/data/            # three sample vcards for manual/auto checks
```

---

## 4) Configuration

Path: `~/.config/rldx/config.toml`.

```toml
vdir = "/home/USER/.contacts"          # required: vdir root
fields_first_pane = ["fname","mname","lname","alias","phone","email"]

[keys]
toggle_search = "/"
confirm = "Enter"
quit = "q"
next = "j"
prev = "k"
edit = "e"
photo_fetch = "i"
lang_next = "L"
tab_next = "Tab"
```

Notes:

* `alias` is **computed** (not stored in vCard). Build it from `NICKNAME`, alternate `FN`s, and any `X-*` name-ish fields.
* Unknown keys → warn, ignore.

---

## 5) vdir normalization (first run)

When `rldx` starts and the DB is empty, normalize the vdir:

1. Find all `*.vcf`. If a file contains multiple cards, split them.
2. Ensure **vCard 4.0** in memory. If not 4.0, mark “needs upgrade” (display banner) and skip writing changes for that card.
3. Ensure `UID`. If missing, set a new UUIDv4.
4. **Filename** = short UUID: first **12 hex** of `UID` (collision-check; extend to 16, then 32 if needed).
5. Write each normalized card atomically (see §8).
6. After success, delete any original multi-card file.
7. Create `.rldx_normalized` marker in the vdir root.

---

## 6) SQLite schema and access

Open DB at `~/.local/share/rldx/index.db` with:

```
PRAGMA journal_mode=WAL;
PRAGMA synchronous=FULL;
PRAGMA foreign_keys=ON;
```

**Tables**:

```sql
CREATE TABLE IF NOT EXISTS items (
  uuid TEXT PRIMARY KEY,            -- full UUID from vCard UID
  path TEXT UNIQUE NOT NULL,        -- absolute .vcf path
  fn   TEXT NOT NULL,               -- chosen display FN
  rev  TEXT,                        -- vCard REV (if any)
  has_photo INTEGER NOT NULL DEFAULT 0,
  has_logo  INTEGER NOT NULL DEFAULT 0,
  sha1 BLOB NOT NULL,               -- of raw .vcf bytes
  mtime INTEGER NOT NULL,           -- seconds since epoch
  lang_pref TEXT                    -- display language chosen for FN
);

CREATE TABLE IF NOT EXISTS props (
  uuid  TEXT NOT NULL REFERENCES items(uuid) ON DELETE CASCADE,
  fn    TEXT NOT NULL,              -- copy of items.fn for quick LIKE
  field TEXT NOT NULL,              -- e.g., FN, NICKNAME, ORG, EMAIL, TEL, ADR, NOTE, ...
  value TEXT NOT NULL,              -- flattened string value
  params TEXT DEFAULT '{}',         -- JSON of parameters (LANGUAGE, PREF, TYPE, ALTID,...)
  seq   INTEGER NOT NULL DEFAULT 0, -- order within same field
  PRIMARY KEY (uuid, field, seq, value)
);

CREATE INDEX IF NOT EXISTS idx_items_fn ON items(fn);
CREATE INDEX IF NOT EXISTS idx_props_field ON props(field);
CREATE INDEX IF NOT EXISTS idx_props_value ON props(value);
CREATE INDEX IF NOT EXISTS idx_props_fn ON props(fn);
```

**Indexing rules**:

* Choose `items.fn` = FN with the lowest `PREF`; on ties, prefer `LANGUAGE` that matches the app’s current language selection; fallback to first FN seen.
* Insert `props` rows for at least: `FN`, `N` (flattened `family;given;...`), `NICKNAME`, `ORG`, `TITLE`, `ROLE`, `EMAIL`, `TEL`, `ADR` (use `LABEL` or a semicolon-joined form), `URL`, `NOTE`, `RELATED`, `PHOTO`, `LOGO`. Include any other parsed fields as discovered.
* Set `has_photo`/`has_logo` if a `PHOTO`/`LOGO` exists (either `data:` or `uri:`).

**Reindex logic**:

* On startup or explicit `--reindex`, for each `.vcf`: compute sha1 and mtime; if changed or unknown, parse and upsert `items` + `props` in one transaction.
* After an edit, update only that contact (no full rescan).

---

## 7) UI behavior (ratatui)

**Overall**: draw four panes. The **first line** shows the **URI** (absolute path as `VDIR://.../<file>.vcf`) and the available languages for `FN` (e.g., `EN | RU`, with the active one highlighted).

**Focus model**: on launch, the **search pane** is open and focused; the first contact in the list is selected. Pressing **Enter** on the search pane closes it and moves focus to the detail panes. Press **/** to reopen.

**Panes**:

1. **Search (left)**: case-insensitive substring over `items.fn` plus `props` where `field IN ('NICKNAME','ORG','EMAIL','TEL')`. Render each hit with an icon (person vs org), `fn`, and either first email or org as a secondary line.

2. **Main card (top center)**: show chosen-language `FN`, structured name (`family/given/middle`), computed `alias` (derived from nicknames + alternate FNs; not stored), and primary phone/email with `[index]` labels.

3. **Image (top right)**: if cached PNG for `uuid` exists, render via kitty. If only `PHOTO:data:` exists, decode to memory, render, and populate cache. Else print “NO IMAGE AVAILABLE”.

4. **Tabs (bottom)**: `Work`, `Personal`, `Accounts`, `Full Metadata`. `Tab` cycles.

   * Work: `ORG`, `TITLE`, `ROLE`, `ADR` type=work, work `EMAIL`/`TEL`.
   * Personal: home `ADR`, personal `EMAIL`/`TEL`, `BDAY`, `ANNIVERSARY`.
   * Accounts: `IMPP` (matrix/xmpp/sip/tg), `URL`, recognizable social `X-*`.
   * Full Metadata: verbatim dump of properties with parameters.

**Keys** (read from config; defaults as shown):

* `/` toggle search pane.
* `Enter` close search (if focused).
* `j/k` move selection.
* `e` edit focused field inline (single-line editor; no new-field modal).
* `i` fetch and embed `PHOTO`/`LOGO` if current card has a URI value.
* `L` cycle display language for this card (affects `items.fn`).
* `Tab` next tab.
* `q` quit.

---

## 8) Atomic writes (strict)

**Write `.vcf` atomically**:

* Write to a temp file in the **same directory**. `O_CREAT|O_EXCL`, write bytes, **fsync(temp)**, `rename(temp → final)`, **fsync(parent dir)**.

**Update DB atomically**:

* `BEGIN IMMEDIATE;` update `items`, delete/insert `props`, `COMMIT;`.
* If any step fails, leave prior state intact.

Always write the file first, then update DB. Never the reverse.

---

## 9) vCard I/O rules

* Version: **must write 4.0**. Refuse to write if the in-memory card cannot be represented as 4.0.
* On save: ensure `UID` (UUIDv4) and update `REV` (UTC `YYYYMMDDTHHMMSSZ`).
* Preserve **unknown** properties and parameters **exactly** (including order) as supported by the vCard crate.
* Implement proper folding (75-octet soft wrap) and escaping of `\, ; \n`—rely on the crate’s writer to do this correctly.
* `KIND` affects UI emphasis only; everything still indexes the same way.
* Language variants: respect `LANGUAGE` and `ALTID`. Keep all variants; set display language by picking the appropriate `FN` when filling `items.fn`.

---

## 10) Editing (existing fields only)

* Inline editor replaces the value cell for the focused field.
* `Enter` saves, `Esc` cancels.
* Save path: update in-memory vCard → `.vcf` atomic write → DB upsert for this uuid.
* Multi-valued fields: the cursor selects a specific instance (`seq` index) to edit.

---

## 11) PHOTO/LOGO fetch and embed

Triggered by key `i`:

* If `PHOTO` or `LOGO` has `VALUE=uri` and `http(s)://`, ask once in the status line: “Fetch and embed? y/N”. On “y”:

  1. GET with `reqwest` (max 10 MB).
  2. Decode with `image`; downscale to max 512×512.
  3. Encode PNG; base64; set `PHOTO:data:image/png;base64,...` (or `LOGO:`).
  4. Save `.vcf` atomically, update DB flags, store PNG cache as `~/.cache/rldx/img/<uuid>.png`.
* For `file://` URIs, read and embed similarly.
* Never auto-fetch remote URIs on view.

---

## 12) CardDAV (libdav) — simple, optional

Feature flag: `--features dav`. Provide a CLI subcommand:

```
rldx --dav-pull --url https://dav.example/abook/ --user USER --pass-command "pass show dav/user"
```

Behavior:

* Connect with libdav, discover addressbook collection, list items with their `href` and `ETag`.
* For each server item: if not present or ETag changed locally, fetch `.vcf` and place into vdir using the normalization filename rule (UID governs).
* Do **not** push in v0. Do **not** run in background.
* After pull, run the regular reindexer.

---

## 13) Program flow

**Startup**:

1. Load config; open DB (create if missing).
2. If no `.rldx_normalized`, run normalization.
3. Reindex changed cards (by sha1+mtime).
4. Launch UI: search pane open and focused; first contact selected.

**On save**: call `save_card(uuid, card)` → atomic file write → `db::upsert(card)`.

**On exit**: close DB cleanly. No background threads remain.

---

## 14) Testing/fixtures and acceptance

Provide three `.vcf` samples in `tests/data/`:

* `person-jane.vcf`: `KIND:individual`, multiple `FN` with `LANGUAGE`, two emails, one mobile, inline `PHOTO:data:`.
* `person-jan.vcf`: `KIND:individual`, nicknames + work/home addresses, `ORG/TITLE/ROLE`.
* `org-janus.vcf`: `KIND:org`, `FN`, `ORG`, work `ADR`, remote `LOGO` URI.

Manual acceptance checklist:

* App starts, shows four panes; first line includes `VDIR://…/<short-uuid>.vcf`.
* `/` focuses search; typing `JAN` filters to Jane + Janus; `Enter` closes search.
* `e` edits an email; save persists to disk and DB; search finds the new email.
* `i` on org with `LOGO:http…` fetches and embeds; image pane updates.
* Language toggle `L` changes which `FN` is displayed in list/header.

---

## 15) Implementation order (step-by-step)

1. Scaffold crate; config loader with defaults; CLI flags (`--reindex`, `--dav-pull` gated behind `feature = "dav"`).
2. DB open + DDL + simple helpers (`upsert_item`, `replace_props`, `query_list`, `query_detail`).
3. vCard I/O using the chosen crate; ensure write emits valid 4.0 and preserves unknowns.
4. vdir scan + normalization + atomic write routine.
5. Indexer: file → sha1/mtime → parse → items/props upsert.
6. UI skeleton drawing four panes; list + detail rendering without editing.
7. Search filter (substring SQL `LIKE` joins) with live narrowing.
8. Inline edit path for a single-value field; then extend to multi-value by `seq`.
9. Photo fetch/embed + kitty image rendering (with a simple capability check).
10. Optional: `--dav-pull` that downloads new/changed cards into vdir and triggers reindex.

Keep the code straightforward and explicit. Prefer clear, small functions over clever abstractions.

