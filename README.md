# rldx

RLDX (pronounced "please-do-not-sue-me") - a TUI attepmt to recreate the ease of use of physical contact managers.

*This is a vibe coded Rust TUI, not even in alpha. Use with appropriate caution.*

## Features

- Browse and search contacts stored as vCard (.vcf) files
- Import contacts from Google Contacts CSV exports
- abook compatible
- Basic contact editor.

## Installation

```bash
cargo build --release
```

Optional feature for WebDAV support:

```bash
cargo build --release --features dav
```

## Usage

```bash
# Launch the TUI
rldx

# Force reindex of all contacts
rldx --reindex

# Import Google Contacts CSV
rldx import --format google contacts.csv

# Query contacts (abook-compatible for mutt/aerc)
rldx query "search term"
```

## Configuration

Create a config file at `~/.config/rldx/config.toml`. See `config.example.toml` for all options.

Minimal configuration:

```toml
vdir = "/path/to/your/vdir"
```

### Key Bindings

Key bindings are fully configurable. Common defaults:

| Key | Action |
|-----|--------|
| `q` | Quit |
| `/` | Search |
| `j`/`k` | Navigate down/up |
| `h`/`l` | Switch panes |
| `e` | Edit field |
| `y` | Copy value |
| `Enter` | Confirm/select |
| `Escape` | Cancel |

## License

See LICENSE file for details.
