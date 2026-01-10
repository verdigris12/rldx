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
use std::path::Path;

use anyhow::Result;
use clap::{Args, Parser, Subcommand, ValueEnum};

use config::Config;
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
