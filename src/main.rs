mod config;
mod crypto;
mod db;
mod import;
mod indexer;
mod search;
mod translit;
mod ui;
mod vcard_io;
mod vdir;

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use age::secrecy::ExposeSecret;
use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};

use config::Config;
use db::Database;

#[derive(Parser, Debug)]
#[command(name = "rldx")]
struct Cli {
    /// Path to configuration file (default: ~/.config/rldx/config.toml)
    #[arg(long, short = 'c', global = true)]
    config: Option<PathBuf>,

    #[arg(long, default_value_t = false)]
    reindex: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand, Debug)]
enum Command {
    Import(ImportArgs),
    /// Query contacts for email addresses (abook-compatible output for aerc/mutt)
    Query(QueryArgs),
    /// Initialize rldx with encryption and create config
    Init(InitArgs),
}

#[derive(Args, Debug)]
struct InitArgs {
    /// Path to vCard storage directory (will be created)
    #[arg(value_name = "PATH")]
    vdir: PathBuf,

    /// Encryption type (required)
    #[arg(long, value_enum)]
    encryption: EncryptionArg,

    /// GPG key ID (required for --encryption gpg)
    #[arg(long)]
    key: Option<String>,

    /// Path to age identity file (optional for --encryption age, generates new if not specified)
    #[arg(long)]
    identity: Option<PathBuf>,

    /// Age recipient public key (optional for --encryption age, derived from identity if not specified)
    #[arg(long)]
    recipient: Option<String>,

    /// Force overwrite existing configuration
    #[arg(long, short = 'f')]
    force: bool,
}

#[derive(Clone, Debug, ValueEnum)]
enum EncryptionArg {
    Gpg,
    Age,
}

#[derive(Args, Debug)]
struct QueryArgs {
    /// Search term (matches name, email, nickname, org)
    query: String,
}

#[derive(Args, Debug)]
struct ImportArgs {
    #[arg(long, value_enum)]
    format: ImportFormat,

    #[arg(long)]
    book: Option<String>,

    /// Auto-merge threshold (0.0-1.0). Contacts with FN similarity
    /// above this threshold will be merged. Recommended: 0.85-0.95
    #[arg(long)]
    automerge: Option<f64>,

    /// Number of threads for parallel processing (maildir only).
    /// Defaults to number of CPU cores.
    #[arg(long, short = 'j')]
    threads: Option<usize>,

    #[arg(value_name = "PATH")]
    input: String,
}

#[derive(Clone, Debug, ValueEnum)]
enum ImportFormat {
    Google,
    Maildir,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Handle commands that don't need config first
    if let Some(ref command) = cli.command {
        if let Command::Init(ref args) = command {
            return handle_init(args, cli.config.as_deref());
        }
    }

    let config = config::load_from(cli.config.as_deref())?;

    // Create the encryption provider
    let provider = crypto::create_provider(&config.encryption)?;

    if let Some(command) = cli.command {
        match command {
            Command::Import(args) => {
                handle_import(args, &config, provider.as_ref())?;
                return Ok(());
            }
            Command::Query(args) => {
                handle_query(args, &config)?;
                return Ok(());
            }
            Command::Init(_) => {
                // Already handled above
                unreachable!();
            }
        }
    }

    println!("Loaded configuration from {}", config.config_path.display());

    let normalize_report = vdir::normalize(&config.vdir, config.phone_region.as_deref(), provider.as_ref())?;
    if !normalize_report.needs_upgrade.is_empty() {
        eprintln!(
            "warning: {} cards require manual upgrade to vCard 4.0",
            normalize_report.needs_upgrade.len()
        );
    }

    // Derive DB key from encryption provider
    let db_key = provider.derive_db_key()?;
    let mut db = Database::open_with_key(Some(&db_key))?;
    reindex(&mut db, &config, cli.reindex, provider.as_ref())?;

    let mut app = ui::app::App::new(&mut db, &config, provider.as_ref())?;
    app.run()?;

    Ok(())
}

fn handle_query(args: QueryArgs, config: &Config) -> Result<()> {
    // Create provider and derive DB key for encrypted database
    let provider = crypto::create_provider(&config.encryption)?;
    let db_key = provider.derive_db_key()?;
    let db = Database::open_with_key(Some(&db_key))?;
    let results = db.query_emails(&args.query)?;

    // Header line (abook-compatible, ignored by mutt/aerc)
    if results.is_empty() {
        println!("No matches for \"{}\"", args.query);
    } else {
        println!(
            "Found {} contact(s) matching \"{}\"",
            results.len(),
            args.query
        );
    }

    // Results: email<TAB>name<TAB>notes (abook mutt-query format)
    for r in results {
        println!(
            "{}\t{}\t{}",
            r.email,
            r.display_fn,
            r.notes.as_deref().unwrap_or(" ")
        );
    }

    Ok(())
}

fn handle_import(args: ImportArgs, config: &Config, provider: &dyn crypto::CryptoProvider) -> Result<()> {
    // Validate automerge threshold
    if let Some(threshold) = args.automerge {
        if !(0.0..=1.0).contains(&threshold) {
            anyhow::bail!("--automerge threshold must be between 0.0 and 1.0");
        }
    }

    let normalize_report = vdir::normalize(&config.vdir, config.phone_region.as_deref(), provider)?;
    if !normalize_report.needs_upgrade.is_empty() {
        eprintln!(
            "warning: {} cards require manual upgrade to vCard 4.0",
            normalize_report.needs_upgrade.len()
        );
    }

    // Open encrypted database
    let db_key = provider.derive_db_key()?;
    let mut db = Database::open_with_key(Some(&db_key))?;

    match args.format {
        ImportFormat::Google => {
            let result = import::google::import_google_contacts(
                Path::new(&args.input),
                config,
                args.book.as_deref(),
                args.automerge,
                &mut db,
                provider,
            )?;

            println!("Imported {} contacts.", result.imported);

            if !result.merged.is_empty() {
                println!("Auto-merged {} contacts:", result.merged.len());
                for merge in &result.merged {
                    println!(
                        "  {} <{}> -> {} ({:.2})",
                        merge.name, merge.email, merge.merged_into, merge.score
                    );
                }
            }

            if result.skipped > 0 {
                println!(
                    "Skipped {} contacts (duplicate email or conversion error).",
                    result.skipped
                );
            }
        }
        ImportFormat::Maildir => {
            let result = import::maildir::import_maildir(
                Path::new(&args.input),
                config,
                args.book.as_deref(),
                args.automerge,
                args.threads,
                &mut db,
                provider,
            )?;

            println!("Imported {} contacts.", result.imported);

            if !result.merged.is_empty() {
                println!("Auto-merged {} contacts:", result.merged.len());
                for merge in &result.merged {
                    println!(
                        "  {} <{}> -> {} ({:.2})",
                        merge.name, merge.email, merge.merged_into, merge.score
                    );
                }
            }

            if result.skipped > 0 {
                println!(
                    "Skipped {} addresses (no name, too short, or duplicate email).",
                    result.skipped
                );
            }
        }
    };

    reindex(&mut db, config, false, provider)?;
    Ok(())
}

fn reindex(
    db: &mut Database,
    config: &Config,
    force: bool,
    provider: &dyn crypto::CryptoProvider,
) -> Result<()> {
    let files = vdir::list_vcf_files(&config.vdir)?;
    let paths_set: HashSet<_> = files.iter().cloned().collect();
    if force {
        // Nuke DB schema and rebuild from scratch
        db.reset_schema()?;
    }
    let stored = if force { Default::default() } else { db.stored_items()? };

    for path in files {
        let state = vdir::compute_file_state(&path)?;

        // Only check SHA1, ignore mtime (handles rsync, backup restore, etc.)
        let requires_index = if force {
            true
        } else {
            match stored.get(&path) {
                Some(existing) => existing.sha1 != state.sha1,
                None => true,
            }
        };

        if !requires_index {
            continue;
        }

        // Only parse files that need reindexing (decrypt with provider)
        let parsed = vcard_io::parse_file(&path, config.phone_region.as_deref(), provider)?;
        let cards = parsed.cards;

        if cards.is_empty() {
            eprintln!("warning: file {} contained no vCards", path.display());
            continue;
        }
        if cards.len() > 1 {
            eprintln!(
                "warning: file {} contained {} cards; indexing the first",
                path.display(),
                cards.len()
            );
        }

        // If parsing normalized the file, recompute state for accurate DB storage
        let final_state = if parsed.changed {
            vdir::compute_file_state(&path)?
        } else {
            state
        };

        let card = cards.into_iter().next().unwrap();
        let record = indexer::build_record(&path, &card, &final_state, None)?;
        db.upsert(&record.item, &record.props)?;
    }

    db.remove_missing(&paths_set)?;
    Ok(())
}

fn handle_init(args: &InitArgs, custom_config_path: Option<&Path>) -> Result<()> {
    let config_path = match custom_config_path {
        Some(p) => config::expand_tilde(p),
        None => config::config_path()?,
    };
    let config_dir = config_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("config path has no parent directory"))?
        .to_path_buf();

    // 1. Check if config already exists
    if config_path.exists() && !args.force {
        bail!(
            "Configuration already exists at {}\n\
             Remove it first or use --force to overwrite.",
            config_path.display()
        );
    }

    // 2. Validate encryption arguments
    let (encryption_type, encryption_section) = match args.encryption {
        EncryptionArg::Gpg => {
            let key = args.key.as_ref().ok_or_else(|| {
                anyhow::anyhow!("--key is required for GPG encryption")
            })?;

            // Verify GPG key exists
            let output = std::process::Command::new("gpg")
                .args(["--list-keys", key])
                .output()
                .context("failed to execute gpg - is GPG installed?")?;

            if !output.status.success() {
                bail!(
                    "GPG key '{}' not found. Make sure the key is imported.\n\
                     Run 'gpg --list-keys' to see available keys.",
                    key
                );
            }

            let section = format!(
                r#"[encryption]
type = "gpg"
gpg_key_id = "{}""#,
                key
            );
            ("gpg", section)
        }
        EncryptionArg::Age => {
            // Handle identity and recipient
            let (identity_path, recipient) = if let Some(ref identity) = args.identity {
                // Use provided identity file
                if !identity.exists() {
                    bail!("age identity file not found: {}", identity.display());
                }
                let recipient = if let Some(ref r) = args.recipient {
                    r.clone()
                } else {
                    // Derive recipient from identity file
                    derive_age_recipient(identity)?
                };
                (identity.clone(), recipient)
            } else {
                // Generate new identity
                if args.recipient.is_some() {
                    bail!("--recipient cannot be specified without --identity");
                }

                let identity_path = config_dir.join("age-identity.txt");
                let (identity_content, recipient) = generate_age_identity()?;

                // Ensure config dir exists for identity file
                fs::create_dir_all(&config_dir)
                    .with_context(|| format!("failed to create config dir: {}", config_dir.display()))?;

                fs::write(&identity_path, &identity_content)
                    .with_context(|| format!("failed to write identity file: {}", identity_path.display()))?;

                // Set restrictive permissions on Unix
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let perms = std::fs::Permissions::from_mode(0o600);
                    fs::set_permissions(&identity_path, perms)?;
                }

                println!("Generated age identity at: {}", identity_path.display());
                (identity_path, recipient)
            };

            let section = format!(
                r#"[encryption]
type = "age"
age_identity = "{}"
age_recipient = "{}""#,
                identity_path.display(),
                recipient
            );
            ("age", section)
        }
    };

    // 3. Validate vdir path (expand tilde)
    let vdir = config::expand_tilde(&args.vdir);
    if vdir.exists() {
        // Check if directory is empty
        let entries: Vec<_> = fs::read_dir(&vdir)
            .with_context(|| format!("failed to read directory: {}", vdir.display()))?
            .collect();

        if !entries.is_empty() {
            bail!(
                "Directory already exists and is not empty: {}\n\
                 Choose a different path or empty the directory first.",
                vdir.display()
            );
        }
    }

    // 4. Create directories
    fs::create_dir_all(&config_dir)
        .with_context(|| format!("failed to create config dir: {}", config_dir.display()))?;

    fs::create_dir_all(&vdir)
        .with_context(|| format!("failed to create vdir: {}", vdir.display()))?;

    // 5. Generate and write config.toml
    let config_content = generate_config_file(&vdir, &encryption_section)?;
    fs::write(&config_path, &config_content)
        .with_context(|| format!("failed to write config file: {}", config_path.display()))?;

    // Set restrictive permissions on config file
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        fs::set_permissions(&config_path, perms)?;
    }

    // 6. Print success message
    println!();
    println!("Initialized rldx with {} encryption.", encryption_type);
    println!();
    println!("Configuration: {}", config_path.display());
    println!("vCard storage: {}", vdir.display());
    println!();
    println!("Run 'rldx' to start the application.");

    Ok(())
}

/// Generate a new age identity and return (file_content, recipient)
fn generate_age_identity() -> Result<(String, String)> {
    let identity = age::x25519::Identity::generate();
    let recipient = identity.to_public();

    let content = format!(
        "# rldx age identity - created {}\n# public key: {}\n{}\n",
        chrono_lite::now(),
        recipient,
        identity.to_string().expose_secret(),
    );

    Ok((content, recipient.to_string()))
}

/// Derive the age recipient (public key) from an identity file
fn derive_age_recipient(identity_path: &Path) -> Result<String> {
    let content = fs::read_to_string(identity_path)
        .with_context(|| format!("failed to read identity file: {}", identity_path.display()))?;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with("AGE-SECRET-KEY-") {
            let identity: age::x25519::Identity = line
                .parse()
                .map_err(|e| anyhow::anyhow!("failed to parse age identity: {}", e))?;
            return Ok(identity.to_public().to_string());
        }
    }

    bail!("no valid age identity found in {}", identity_path.display())
}

/// Generate the config.toml content
fn generate_config_file(vdir: &Path, encryption_section: &str) -> Result<String> {
    // Canonicalize the vdir path for cleaner output
    let vdir_str = vdir.canonicalize()
        .unwrap_or_else(|_| vdir.to_path_buf())
        .display()
        .to_string();

    let config = format!(
        r##"# rldx configuration
# Generated by 'rldx init'

# Path to the directory containing your vCard files.
vdir = "{vdir}"

# =============================================================================
# Encryption (required)
# =============================================================================
# All vCard files are encrypted. Supported backends:
#   gpg - GPG encryption (uses gpg-agent, files stored as .vcf.gpg)
#   age - Age encryption (modern alternative, files stored as .vcf.age)

{encryption}

# =============================================================================
# Top Bar Buttons
# =============================================================================
# Map function keys (F1-F12) to actions displayed in the header bar.
# Supported actions: help, edit, refresh, share

[top_bar]
F1 = "help"
F3 = "edit"
F5 = "refresh"
F7 = "share"

# Optional: override which fields appear in the first pane.
# fields_first_pane = ["fname", "mname", "lname", "alias", "phone", "email"]

# Optional: default region for phone number normalization (ISO 3166-1 alpha-2).
# phone_region = "US"

# =============================================================================
# Key Bindings
# =============================================================================
# Keys are organized by context. Each action can have multiple bindings.
# Supported key names:
#   - Single characters: "a", "A", "/", "?", etc. (case-sensitive)
#   - Special keys: "Enter", "Escape", "Tab", "Backtab", "Space", "Backspace"
#   - Arrow keys: "Up", "Down", "Left", "Right"
#   - Navigation: "PageUp", "PageDown", "Home", "End"
#   - Function keys: "F1" through "F12"

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

# =============================================================================
# UI Configuration
# =============================================================================

[ui.colors]
border = [255, 140, 0]
selection_bg = [255, 140, 0]
selection_fg = [0, 0, 0]
separator = [255, 140, 0]
status_fg = [255, 140, 0]
status_bg = [0, 0, 0]

[ui.icons]
address_book = "@"
contact = "👤 "
organization = "🏢 "

[ui.pane.image]
width = 40
height = 12

# =============================================================================
# Commands
# =============================================================================

[commands]
# Program that receives copied values on stdin.
# Examples:
#   copy = ["wl-copy"]                           # Wayland
#   copy = ["xclip", "-selection", "clipboard"]  # X11
#   copy = ["pbcopy"]                            # macOS
copy = ["wl-copy"]

# =============================================================================
# Maildir Import Filters
# =============================================================================
# Filters for 'rldx import --format maildir' command.

[maildir_import]
skip_local_patterns = [
    "noreply", "no-reply", "no_reply",
    "donotreply", "do-not-reply", "do_not_reply",
    "notifications", "notification",
    "mailer-daemon", "postmaster", "bounce",
    "auto-reply", "autoreply", "automated"
]

skip_domains = [
    "facebookmail.com", "*.facebookmail.com",
    "linkedin.com", "*.linkedin.com",
    "amazonses.com", "*.amazonses.com",
    "sendgrid.net", "*.sendgrid.net",
    "mailchimp.com", "*.mailchimp.com",
    "mailgun.org", "*.mailgun.org",
]

simhash_threshold = 4
min_name_length = 8
min_fn_spaces = 1
email_entropy_threshold = 3.5
"##,
        vdir = vdir_str,
        encryption = encryption_section
    );

    Ok(config)
}

// Simple date/time helper (avoid heavy chrono dependency)
mod chrono_lite {
    use std::time::{SystemTime, UNIX_EPOCH};

    pub fn now() -> String {
        let duration = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let secs = duration.as_secs();

        // Simple UTC timestamp
        let days = secs / 86400;
        let remaining = secs % 86400;
        let hours = remaining / 3600;
        let minutes = (remaining % 3600) / 60;
        let seconds = remaining % 60;

        // Days since 1970-01-01
        // This is a simplified calculation, good enough for logging
        let (year, month, day) = days_to_ymd(days as i64);

        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
            year, month, day, hours, minutes, seconds
        )
    }

    fn days_to_ymd(days: i64) -> (i64, u32, u32) {
        // Simplified algorithm, may be off by a day in some cases
        let mut remaining = days;
        let mut year = 1970;

        loop {
            let days_in_year = if is_leap_year(year) { 366 } else { 365 };
            if remaining < days_in_year {
                break;
            }
            remaining -= days_in_year;
            year += 1;
        }

        let leap = is_leap_year(year);
        let months = [
            31,
            if leap { 29 } else { 28 },
            31,
            30,
            31,
            30,
            31,
            31,
            30,
            31,
            30,
            31,
        ];

        let mut month = 1;
        for days_in_month in months {
            if remaining < days_in_month {
                break;
            }
            remaining -= days_in_month;
            month += 1;
        }

        (year, month, (remaining + 1) as u32)
    }

    fn is_leap_year(year: i64) -> bool {
        (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
    }
}
