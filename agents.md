# rldx - Detailed Build Specification

rldx is a terminal PIM for browsing and managing vCard 4.0 contact collections stored as a **vdir** (one `.vcf` per contact). It uses a **SQLite** index for fast listing/search, renders images via the **kitty** graphics protocol, and provides inline editing with atomic writes. The vdir is canonical; the DB is a rebuildable cache.

---

## 1) Project Definition

**Implemented in v0:**

- Read/write vCard **4.0** only (via `vcard4` crate)
- Normalize vdir to "one contact per file; file named by UUID"
- SQLite index with fluid schema for properties
- Four-pane Ratatui UI: Search, Main Card, Image, Tabs
- Inline editing of existing fields with atomic save
- Contact merging (mark multiple contacts, merge inductively)
- Multi-value modals for email/phone fields
- Phone number normalization to E.164 (via `rlibphonenumber`)
- Photo/logo display via kitty graphics protocol
- Google Contacts import (vCard 3.0 -> 4.0 conversion)
- Maildir import (extract contacts from email headers)
- Configurable keybindings, colors, and field layout
- Age and GPG encryption for vCard files (`.vcf.age`, `.vcf.gpg`)
- SQLCipher encryption for database
- CardDAV sync (bidirectional, via libdav 0.10)
- Automated tests (34 unit + 14 integration)

**Not yet implemented:**

- Photo fetching from URLs (scaffolded but not wired)

**Binary name:** `rldx`

---

## 2) Tech and Dependencies

```toml
# Core
ratatui = "0.28.1"           # TUI framework
crossterm = "0.27.0"         # Terminal handling
rusqlite = "0.31.0"          # SQLite (bundled)
vcard4 = "0.7.2"             # vCard 4.0 parsing/writing

# UI extensions
ratatui-image = "0.7"        # Image rendering (kitty protocol)
tui-input = "0.8"            # Text input widget
tui-widgets = "0.3"          # Popup dialogs

# Data handling
serde = "1.0"                # Serialization
serde_json = "1.0"           # JSON for params storage
toml = "0.8.14"              # Config parsing

# Utilities
clap = "4.5.4"               # CLI argument parsing
anyhow = "1.0.86"            # Error handling
uuid = "1.8.0"               # UUID generation
sha1 = "0.10.6"              # File hashing for change detection
time = "0.3.36"              # Timestamps
base64 = "0.22.1"            # Photo encoding
image = "0.24.9"             # Photo decoding
rlibphonenumber = "0.2.3"    # Phone normalization
directories = "5.0.1"        # XDG paths

# Encryption
age = "0.11"                 # Age encryption
gpgme = "0.11"               # GPG encryption (optional)
hex = "0.4"                  # Hex encoding for DB keys

# CardDAV
libdav = "0.10"              # CardDAV client
hyper-rustls = "0.27"        # HTTPS support
tower-http = "0.6"           # HTTP middleware (auth)
tokio = "1.0"                # Async runtime
```

---

## 3) Repository Layout

```
rldx/
  Cargo.toml
  src/
    main.rs              # CLI entry + app startup + reindex logic
    config.rs            # Load/validate TOML config
    crypto.rs            # Age/GPG encryption providers
    vcard_io.rs          # vCard read/write using vcard4 crate
    vdir.rs              # Scan, normalize, atomic file writes
    db.rs                # SQLite schema, CRUD, queries
    search.rs            # Query normalization helpers
    indexer.rs           # Build IndexedItem/IndexedProp from vCard
    sync.rs              # CardDAV sync engine
    translit.rs          # Transliteration for non-Latin scripts
    ui/
      mod.rs             # Module exports
      app.rs             # App state + event loop (~2200 lines)
      draw.rs            # Layout + rendering (~850 lines)
      panes.rs           # Detail tab definitions (Work, Personal, Accounts, Metadata)
      edit.rs            # Inline edit widget
    import/
      mod.rs             # Module exports
      google.rs          # Google Contacts vCard 3.0 -> 4.0 converter
      maildir.rs         # Maildir email header extraction
      simhash_index.rs   # SimHash-based fuzzy matching for dedup
    remote/
      mod.rs             # Remote trait and types
      carddav.rs         # CardDAV client implementation (libdav)
  tests/
    integration.rs       # Integration tests
    test_contacts.vcf    # Sample Google Contacts export
    test_maildir/        # Sample maildir for testing
```

---

## 4) Configuration

**Path:** `~/.config/rldx/config.toml`

```toml
vdir = "/home/USER/.contacts"          # required
db_path = "/home/USER/.local/share/rldx/index.db"  # optional

# Encryption (required)
[encryption]
type = "age"                           # "age" or "gpg"
age_identity = "/path/to/age-identity.txt"
age_recipient = "age1..."
# OR for GPG:
# type = "gpg"
# gpg_key_id = "0x..."

# phone_region = "US"                  # optional, for phone normalization

# Key bindings are organized by context
# Each action can have multiple bindings (as array or single string)
# Bindings within a context must not collide

[keys.global]
quit = ["q"]
search = ["/"]
help = ["F1", "?"]

[keys.search_input]
cancel = ["Escape"]
confirm = ["Enter"]

[keys.search_results]
cancel = ["Escape"]
confirm = ["Enter"]
next = ["j", "Down", "Tab"]
prev = ["k", "Up", "Backtab"]
page_down = ["PageDown"]
page_up = ["PageUp"]
mark = ["Space"]
merge = ["m"]
toggle_marked = ["M"]

[keys.navigation]
next = ["j", "Down", "Tab"]
prev = ["k", "Up", "Backtab"]
tab_next = ["l", "Right"]
tab_prev = ["h", "Left"]
edit = ["e"]
copy = ["y", "Space"]
confirm = ["Enter"]
add_alias = ["a"]
photo_fetch = ["i"]
lang_cycle = ["L"]

[keys.modal]
cancel = ["Escape", "q"]
confirm = ["Enter", "y"]
next = ["j", "Down", "Tab"]
prev = ["k", "Up", "Backtab"]
edit = ["e"]
copy = ["y", "Space"]
set_default = ["d"]

[keys.editor]
cancel = ["Escape"]
confirm = ["Enter"]

[ui]
# Color customization available

[commands]
copy = ["wl-copy"]  # or ["xclip", "-selection", "clipboard"]

# CardDAV remotes (optional, multiple allowed)
[[remotes]]
name = "fastmail"
url = "https://carddav.fastmail.com"
username = "user@example.com"
password_cmd = "pass show fastmail/carddav"  # command to get password
address_book = "Default"                      # address book name on server
local_book = "fastmail"                       # optional: sync to subdirectory
conflict_prefer = "theirs"                    # "ours" or "theirs"
```

**Supported key names:**
- Single characters: `"a"`, `"A"`, `"/"`, `"?"`, etc. (case-sensitive: `"m"` != `"M"`)
- Special keys: `"Enter"`, `"Escape"`, `"Tab"`, `"Backtab"`, `"Space"`, `"Backspace"`
- Arrow keys: `"Up"`, `"Down"`, `"Left"`, `"Right"`
- Navigation: `"PageUp"`, `"PageDown"`, `"Home"`, `"End"`
- Function keys: `"F1"` through `"F12"`

**Notes:**

- Single-character bindings are CASE-SENSITIVE (`"M"` requires Shift, different from `"m"`)
- Ctrl/Alt/Super modifiers are NOT supported
- Unknown keys log a warning and are ignored
- Key binding collisions within a context cause startup failure
- `fields_first_pane` controls which fields appear in the main card pane
- `phone_region` sets default region for phone normalization

---

## 5) vdir Normalization (First Run)

When rldx starts and no `.rldx_normalized` marker exists:

1. Find all `*.vcf` files recursively
2. If a file contains multiple cards, split them
3. Ensure vCard 4.0 format (convert if needed)
4. Ensure `UID` exists; if missing, generate UUIDv4
5. Rename file to `<uuid>.vcf`
6. Write each normalized card atomically
7. Delete original multi-card files after successful split
8. Create `.rldx_normalized` marker in vdir root

---

## 6) SQLite Schema

**Location:** `~/.local/share/rldx/index.db` (encrypted with SQLCipher)

**Pragmas:**
```sql
PRAGMA key = 'x<derived-key>';  -- SQLCipher encryption
PRAGMA journal_mode=WAL;
PRAGMA synchronous=FULL;
PRAGMA foreign_keys=ON;
```

**Tables:**

```sql
CREATE TABLE IF NOT EXISTS items (
  uuid      TEXT PRIMARY KEY,
  path      TEXT UNIQUE NOT NULL,
  fn        TEXT NOT NULL,          -- display name (chosen FN)
  fn_norm   TEXT,                   -- normalized for search
  fn_simhash INTEGER,               -- SimHash for fuzzy matching
  rev       TEXT,                   -- vCard REV timestamp
  has_photo INTEGER NOT NULL DEFAULT 0,
  has_logo  INTEGER NOT NULL DEFAULT 0,
  sha1      BLOB NOT NULL,          -- file content hash
  mtime     INTEGER NOT NULL,       -- file modification time
  lang_pref TEXT                    -- preferred language for FN
);

CREATE TABLE IF NOT EXISTS props (
  uuid   TEXT NOT NULL REFERENCES items(uuid) ON DELETE CASCADE,
  fn     TEXT NOT NULL,             -- copy for quick LIKE
  field  TEXT NOT NULL,             -- FN, TEL, EMAIL, ORG, etc.
  value  TEXT NOT NULL,
  value_norm TEXT,                  -- normalized for search
  params TEXT DEFAULT '{}',         -- JSON of vCard parameters
  seq    INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (uuid, field, seq, value)
);

-- SimHash index for fuzzy name matching
CREATE TABLE IF NOT EXISTS simhashes (
  uuid TEXT NOT NULL REFERENCES items(uuid) ON DELETE CASCADE,
  simhash INTEGER NOT NULL,
  source TEXT NOT NULL,             -- 'fn', 'nickname', etc.
  value_norm TEXT NOT NULL,
  PRIMARY KEY (uuid, simhash, source, value_norm)
);

-- Sync metadata for CardDAV remotes
CREATE TABLE IF NOT EXISTS sync_metadata (
  contact_path TEXT NOT NULL,
  remote_name TEXT NOT NULL,
  remote_href TEXT NOT NULL,
  remote_etag TEXT,
  last_synced INTEGER,
  local_modified INTEGER DEFAULT 0,
  PRIMARY KEY (contact_path, remote_name)
);
```

**Indexed fields:** FN, N, NICKNAME, ORG, TITLE, ROLE, EMAIL, TEL, ADR, URL, NOTE, RELATED, PHOTO, LOGO, BDAY, ANNIVERSARY, CATEGORIES, GENDER, IMPP, MEMBER, KIND, plus any X-* extensions.

**Reindex logic:**

- On startup: for each `.vcf`, compute sha1+mtime; if changed or unknown, parse and upsert
- `--reindex` flag forces full re-read regardless of hashes
- After edit: update only that contact's records

---

## 7) UI Behavior

### Layout

```
+------------------+------------------------+--------+
| SEARCH PANE      | MAIN CARD              | IMAGE  |
| (contact list)   | (FN, name, phone,      |        |
|                  |  email, aliases)       |        |
+------------------+------------------------+--------+
| DETAIL TABS                                        |
| [Work] [Personal] [Accounts] [Metadata]            |
+----------------------------------------------------+
| STATUS BAR (help, editing status, etc.)            |
+----------------------------------------------------+
```

**Header:** Shows vdir path and available languages for current contact.

### Focus Model

- On launch: search pane open and focused, first contact selected
- `/` opens/focuses search input
- `Enter` on search results closes search, moves focus to detail panes
- `Escape` closes search or cancels current operation
- `Tab` cycles through detail tabs

### Panes

1. **Search (left):** Case-insensitive substring over `items.fn` plus props (NICKNAME, ORG, EMAIL, TEL). Shows icon (person/org), display name, secondary line (org or email).

2. **Main Card (top center):** Chosen-language FN, structured name components, aliases (computed from NICKNAME + alternate FNs), primary phone/email with index labels.

3. **Image (top right):** Renders PHOTO/LOGO via kitty protocol if available; otherwise shows placeholder.

4. **Detail Tabs (bottom):**
   - **Work:** ORG, TITLE, ROLE, work ADR/EMAIL/TEL
   - **Personal:** home ADR, personal EMAIL/TEL, BDAY, ANNIVERSARY
   - **Accounts:** IMPP, URL, social X-* fields
   - **Metadata:** Verbatim property dump with parameters

### Key Bindings

Key bindings are context-aware. Defaults include both vim-style and standard keys.

**Global (work everywhere):**
| Key | Action |
|-----|--------|
| `q` | Quit |
| `/` | Open/focus search |
| `F1`, `?` | Help (not yet implemented) |

**Search Input (typing in search box):**
| Key | Action |
|-----|--------|
| `Escape` | Move focus to results |
| `Enter` | Select contact and close search |

**Search Results (navigating result list):**
| Key | Action |
|-----|--------|
| `j`, `Down`, `Tab` | Next result |
| `k`, `Up`, `Backtab` | Previous result |
| `PageDown` | Jump down 5 |
| `PageUp` | Jump up 5 |
| `Space` | Mark/unmark for merge |
| `m` | Merge marked contacts |
| `M` | Toggle marked-only view |
| `Enter` | Select and close search |
| `Escape` | Close search |

**Navigation (card/detail panes):**
| Key | Action |
|-----|--------|
| `j`, `Down`, `Tab` | Next field |
| `k`, `Up`, `Backtab` | Previous field |
| `l`, `Right` | Next pane/tab |
| `h`, `Left` | Previous pane/tab |
| `e` | Edit current field |
| `y`, `Space` | Copy current field |
| `Enter` | Open multivalue modal |
| `a` | Add alias (when ALIAS focused) |
| `i` | Fetch photo (not implemented) |
| `L` | Cycle language (not implemented) |
| `1-5` | Jump to pane by number |

**Modal Dialogs:**
| Key | Action |
|-----|--------|
| `Escape`, `q` | Close modal |
| `Enter`, `y` | Confirm |
| `j`, `Down`, `Tab` | Next item |
| `k`, `Up`, `Backtab` | Previous item |
| `e` | Edit selected |
| `y`, `Space` | Copy and close |
| `d` | Set as default |

**Inline Editor:**
| Key | Action |
|-----|--------|
| `Escape` | Cancel edit |
| `Enter` | Save edit |

---

## 8) Atomic Writes

**File writes:**
1. Write to temp file in same directory (`O_CREAT|O_EXCL`)
2. Write content
3. `fsync(temp_file)`
4. `rename(temp -> final)`
5. `fsync(parent_directory)`

**Database updates:**
1. `BEGIN IMMEDIATE`
2. Update items table
3. Delete old props, insert new props
4. `COMMIT`

**Order:** Always write file first, then update DB.

---

## 9) vCard I/O Rules

- **Version:** Must write vCard 4.0
- **On save:** Ensure UID (UUIDv4), update REV timestamp
- **Preservation:** Unknown properties and parameters preserved exactly
- **Folding/escaping:** Handled by vcard4 crate
- **Language variants:** Respect LANGUAGE and ALTID parameters; keep all variants; choose display FN based on preference

---

## 10) Inline Editing

- `e` starts editing the focused field
- Editor replaces the value cell inline
- `Enter` saves, `Escape` cancels
- **Save path:** Update vCard in memory -> atomic file write -> DB upsert
- Status bar shows "EDITING $FIELD. ESCAPE TO CANCEL."
- Multi-valued fields: cursor selects specific instance (by seq) to edit

---

## 11) Multi-Value Modals

For EMAIL and TEL fields with multiple values:

- `Enter` on a multi-value field opens modal
- Modal shows table: [value, type]
- `j/k` or `Tab/Backtab` selects row
- `d` sets selected value as default (PREF=1, moves to first position)
- `Space` or `y` copies value and closes modal
- `Escape` or `q` closes modal
- Status bar shows modal-specific help

---

## 12) Contact Merging

**Trigger:** Mark contacts with `Space`, press `m` to merge.

**Strategy** (see `merge-strategy.md` for full details):

- Contacts merged inductively (C + D1 -> C', C' + D2 -> C'', etc.)
- First contact is canonical; keeps its UID
- **Name:** Keep canonical FN; use more complete N if available
- **Multi-valued (TEL, EMAIL, IMPP, URL, ADR, NICKNAME):** Union with deduplication
- **TEL/EMAIL:** Normalize before dedup; union TYPE parameters; single PREF=1
- **Scalars (ORG, TITLE, BDAY, etc.):** Prefer canonical unless donor is more complete
- **PHOTO/LOGO:** Prefer higher resolution; dedup by hash
- **Unknown/X-*:** Preserve all
- **Post-merge:** Update REV, ensure PREF integrity, delete donor files

---

## 13) Import

### Google Contacts

```bash
rldx import --format google contacts.vcf [--book subdir]
```

- Converts Google Contacts vCard 3.0 export to vCard 4.0
- Handles quoted-printable encoding, base64 photos
- Assigns new UUIDs, updates REV timestamps
- Saves to configured vdir (or `--book` subdirectory)

### Maildir

```bash
rldx import --format maildir ~/Mail/INBOX [--book subdir]
```

- Extracts contacts from email headers (From, To, Cc)
- Uses SimHash-based fuzzy matching to avoid duplicates
- Filters out noreply addresses, high-entropy strings, and local domains
- Configurable skip patterns in config.toml

---

## 14) Program Flow

**Startup:**
1. Parse CLI args (clap)
2. Load config from `~/.config/rldx/config.toml`
3. Validate key bindings (error on collisions within context)
4. If no `.rldx_normalized`, run normalization
5. Open SQLite database (create if missing)
6. Reindex changed cards (by sha1+mtime)
7. Launch TUI: search pane focused, first contact selected

**On edit:**
1. User presses `e` on focused field
2. Inline editor activated
3. On `Enter`: parse vCard, update field, write atomically, upsert DB
4. Refresh display

**On merge:**
1. User marks contacts with `Space`
2. User presses `m`
3. Merge contacts inductively per strategy
4. Write merged card, delete donors
5. Reindex, refresh display

**On exit:**
- Close DB cleanly
- No background threads remain

---

## 15) Key Data Types

```rust
// db.rs
struct IndexedItem {
    uuid: String,
    path: PathBuf,
    display_fn: String,
    rev: Option<String>,
    has_photo: bool,
    has_logo: bool,
    sha1: Vec<u8>,
    mtime: i64,
    lang_pref: Option<String>,
}

struct IndexedProp {
    field: String,      // FN, TEL, EMAIL, etc.
    value: String,
    params: Value,      // JSON
    seq: i64,
}

struct ContactListEntry {
    uuid: String,
    display_fn: String,
    path: PathBuf,
    primary_org: Option<String>,
    kind: Option<String>,
}

// ui/app.rs
enum PaneFocus { Search, Card, Detail(DetailTab) }
enum DetailTab { Work, Personal, Accounts, Metadata }
enum MultiValueField { Email, Phone }

struct PaneField {
    label: String,
    value: String,
    copy_value: String,
    source: Option<FieldRef>,
}
```

---

## 16) CardDAV Sync

### Commands

```bash
rldx sync <remote>              # Full bidirectional sync
rldx sync <remote> --pull-only  # Download only
rldx sync <remote> --dry-run    # Preview changes

rldx remote add <name> --url <url> --username <user>
rldx remote list
rldx remote remove <name> [--purge]
rldx remote test <name>
```

### Sync Behavior

**Pull phase:**
1. List all remote contacts (PROPFIND with etags)
2. Compare etags with stored sync_metadata
3. Download new/changed contacts in batches of 50 (addressbook-multiget)
4. Handle remote deletions (remove local files if remote contact gone)

**Push phase:**
1. List local vCard files
2. Compare mtime with last_synced in sync_metadata
3. Upload new/modified contacts (PUT with text/vcard)
4. Delete remote contacts for locally-deleted files

**Conflict resolution:**
- Configurable via `conflict_prefer` in remote config
- `theirs`: Remote wins (default)
- `ours`: Local wins

**File naming:**
- Local files named by UUID extracted from vCard UID
- Avoids metadata leakage from remote filenames

### Implementation

Uses libdav 0.10 with:
- `CardDavClient::bootstrap_via_service_discovery()` for auto-discovery
- `FindAddressBookHomeSet` + `FindAddressBooks` to locate address book
- `GetAddressBookResources` for listing and multiget
- `PutResource` for uploads
- `Delete` for deletions

---

## 17) Future Work

**Photo fetch (scaffolded):**
- Key `i` should trigger fetch from PHOTO/LOGO URI
- GET with reqwest, downscale to 512x512, encode PNG, embed as data: URI
- Confirm before fetching remote URLs

---

## 18) Code Style Guidelines

- Prefer explicit, straightforward code over clever abstractions
- Use `anyhow::Result` for fallible operations
- Use `thiserror` for custom error types
- Heavy use of `Option` and pattern matching
- Keep UI rendering (`draw.rs`) separate from state management (`app.rs`)
- Atomic writes for all mutations
- Preserve vCard data fidelity; don't drop unknown fields
- Key handling uses `key_matches_any()` for multi-binding support
- Key bindings organized by context to avoid collisions
