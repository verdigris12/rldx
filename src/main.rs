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
use std::path::Path;

use age::secrecy::ExposeSecret;
use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};

use config::{Config, EncryptionType};
use db::Database;

#[derive(Parser, Debug)]
#[command(name = "rldx")]
struct Cli {
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
    /// Initialize encryption (generate age keys)
    Init(InitArgs),
    /// Encrypt existing plaintext vCards (migration)
    Encrypt(EncryptArgs),
}

#[derive(Args, Debug)]
struct InitArgs {
    /// Encryption type to initialize
    #[arg(long, value_enum, default_value = "age")]
    encryption: InitEncryption,

    /// Force overwrite existing identity file
    #[arg(long, short = 'f')]
    force: bool,
}

#[derive(Clone, Debug, ValueEnum)]
enum InitEncryption {
    /// Generate age identity (recommended)
    Age,
    /// Show GPG configuration help
    Gpg,
}

#[derive(Args, Debug)]
struct EncryptArgs {
    /// Encryption type to use (must match config)
    #[arg(long, value_enum)]
    encryption: EncryptionArg,

    /// Dry run - show what would be done without making changes
    #[arg(long)]
    dry_run: bool,
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
            return handle_init(args);
        }
    }

    let config = config::load()?;

    if let Some(command) = cli.command {
        match command {
            Command::Import(args) => {
                handle_import(args, &config)?;
                return Ok(());
            }
            Command::Query(args) => {
                handle_query(args)?;
                return Ok(());
            }
            Command::Init(_) => {
                // Already handled above
                unreachable!();
            }
            Command::Encrypt(args) => {
                handle_encrypt(args, &config)?;
                return Ok(());
            }
        }
    }

    println!("Loaded configuration from {}", config.config_path.display());

    let normalize_report = vdir::normalize(&config.vdir, config.phone_region.as_deref())?;
    if !normalize_report.needs_upgrade.is_empty() {
        eprintln!(
            "warning: {} cards require manual upgrade to vCard 4.0",
            normalize_report.needs_upgrade.len()
        );
    }

    let mut db = Database::open()?;
    reindex(&mut db, &config, cli.reindex)?;

    let mut app = ui::app::App::new(&mut db, &config)?;
    app.run()?;

    Ok(())
}

fn handle_query(args: QueryArgs) -> Result<()> {
    let db = Database::open()?;
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

fn handle_import(args: ImportArgs, config: &Config) -> Result<()> {
    // Validate automerge threshold
    if let Some(threshold) = args.automerge {
        if !(0.0..=1.0).contains(&threshold) {
            anyhow::bail!("--automerge threshold must be between 0.0 and 1.0");
        }
    }

    let normalize_report = vdir::normalize(&config.vdir, config.phone_region.as_deref())?;
    if !normalize_report.needs_upgrade.is_empty() {
        eprintln!(
            "warning: {} cards require manual upgrade to vCard 4.0",
            normalize_report.needs_upgrade.len()
        );
    }

    let mut db = Database::open()?;

    match args.format {
        ImportFormat::Google => {
            let result = import::google::import_google_contacts(
                Path::new(&args.input),
                config,
                args.book.as_deref(),
                args.automerge,
                &mut db,
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

    reindex(&mut db, config, false)?;
    Ok(())
}

fn reindex(db: &mut Database, config: &Config, force: bool) -> Result<()> {
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

        // Only parse files that need reindexing
        let parsed = vcard_io::parse_file(&path, config.phone_region.as_deref())?;
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

fn handle_init(args: &InitArgs) -> Result<()> {
    match args.encryption {
        InitEncryption::Age => init_age(args.force),
        InitEncryption::Gpg => init_gpg_help(),
    }
}

fn init_age(force: bool) -> Result<()> {
    // Ensure config directory exists
    config::ensure_config_dir()?;
    let config_dir = config::config_path()?.parent().unwrap().to_path_buf();
    let identity_path = config_dir.join("age-identity.txt");

    if identity_path.exists() && !force {
        anyhow::bail!(
            "age identity already exists at {}\nUse --force to overwrite",
            identity_path.display()
        );
    }

    // Generate new age identity
    let identity = age::x25519::Identity::generate();
    let recipient = identity.to_public();

    // Write identity file
    let identity_content = format!(
        "# rldx age identity - created {}\n# public key: {}\n{}\n",
        chrono_lite::now(),
        recipient,
        identity.to_string().expose_secret(),
    );

    fs::write(&identity_path, identity_content)
        .with_context(|| format!("failed to write identity file: {}", identity_path.display()))?;

    // Set restrictive permissions on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        fs::set_permissions(&identity_path, perms)?;
    }

    println!("Generated age identity at: {}", identity_path.display());
    println!();
    println!("Add the following to your config.toml:");
    println!();
    println!("[encryption]");
    println!("type = \"age\"");
    println!("age_identity = \"{}\"", identity_path.display());
    println!("age_recipient = \"{}\"", recipient);
    println!();
    println!("Then run 'rldx encrypt --encryption age' to encrypt existing vCards.");

    Ok(())
}

fn init_gpg_help() -> Result<()> {
    println!("GPG Encryption Setup");
    println!("====================");
    println!();
    println!("1. Ensure you have a GPG key pair. List your keys with:");
    println!("   gpg --list-keys");
    println!();
    println!("2. If you don't have a key, generate one with:");
    println!("   gpg --full-generate-key");
    println!();
    println!("3. Note your key ID (looks like 0x1234ABCD or a 40-character fingerprint)");
    println!();
    println!("4. Add the following to your config.toml:");
    println!();
    println!("[encryption]");
    println!("type = \"gpg\"");
    println!("gpg_key_id = \"YOUR_KEY_ID_HERE\"");
    println!();
    println!("5. Then run 'rldx encrypt --encryption gpg' to encrypt existing vCards.");
    println!();
    println!("Note: GPG encryption uses gpg-agent for passphrase caching.");
    println!("Make sure gpg-agent is running and configured correctly.");

    Ok(())
}

fn handle_encrypt(args: EncryptArgs, config: &Config) -> Result<()> {
    let target_type = match args.encryption {
        EncryptionArg::Gpg => EncryptionType::Gpg,
        EncryptionArg::Age => EncryptionType::Age,
    };

    // Verify config matches the requested encryption type
    if config.encryption.encryption_type != target_type {
        anyhow::bail!(
            "config.toml encryption type ({:?}) doesn't match requested type ({:?})\n\
             Please update your config.toml first.",
            config.encryption.encryption_type,
            target_type
        );
    }

    // Create the crypto provider
    let provider = crypto::create_provider(&config.encryption)?;

    // Find all plaintext .vcf files
    let vcf_files = vdir::list_vcf_files(&config.vdir)?;

    if vcf_files.is_empty() {
        println!("No plaintext .vcf files found to encrypt.");
        return Ok(());
    }

    println!("Found {} plaintext vCard files to encrypt.", vcf_files.len());

    if args.dry_run {
        println!("\nDry run - would encrypt:");
        for path in &vcf_files {
            let stem = vdir::vcf_base_stem(path).unwrap_or_default();
            let target = vdir::vcf_target_path(&config.vdir, &stem, target_type);
            println!("  {} -> {}", path.display(), target.display());
        }
        return Ok(());
    }

    let mut encrypted = 0;
    let mut errors = 0;

    for path in vcf_files {
        let stem = match vdir::vcf_base_stem(&path) {
            Some(s) => s,
            None => {
                eprintln!("warning: could not determine stem for {}", path.display());
                errors += 1;
                continue;
            }
        };

        // Read plaintext
        let plaintext = match fs::read(&path) {
            Ok(data) => data,
            Err(e) => {
                eprintln!("warning: failed to read {}: {}", path.display(), e);
                errors += 1;
                continue;
            }
        };

        // Encrypt
        let ciphertext = match provider.encrypt(&plaintext) {
            Ok(data) => data,
            Err(e) => {
                eprintln!("warning: failed to encrypt {}: {}", path.display(), e);
                errors += 1;
                continue;
            }
        };

        // Write encrypted file
        let target = vdir::vcf_target_path(&config.vdir, &stem, target_type);
        if let Err(e) = vdir::write_atomic(&target, &ciphertext) {
            eprintln!("warning: failed to write {}: {}", target.display(), e);
            errors += 1;
            continue;
        }

        // Remove original plaintext file
        if let Err(e) = fs::remove_file(&path) {
            eprintln!(
                "warning: failed to remove original {}: {}",
                path.display(),
                e
            );
            // Don't count as error since encryption succeeded
        }

        encrypted += 1;
    }

    println!("\nEncrypted {} files.", encrypted);
    if errors > 0 {
        println!("{} files had errors.", errors);
    }

    // Also encrypt the database
    println!("\nNote: The database will be encrypted on next startup.");
    println!("Run 'rldx --reindex' to force a full reindex with encryption.");

    Ok(())
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
