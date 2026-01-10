mod config;
mod db;
mod import;
mod indexer;
mod search;
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

    #[arg(value_name = "FILE")]
    input: String,
}

#[derive(Clone, Debug, ValueEnum)]
enum ImportFormat {
    Google,
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
    let normalize_report = vdir::normalize(&config.vdir, config.phone_region.as_deref())?;
    if !normalize_report.needs_upgrade.is_empty() {
        eprintln!(
            "warning: {} cards require manual upgrade to vCard 4.0",
            normalize_report.needs_upgrade.len()
        );
    }

    let imported = match args.format {
        ImportFormat::Google => import::google::import_google_contacts(
            Path::new(&args.input),
            config,
            args.book.as_deref(),
        )?,
    };

    println!("Imported {imported} contacts.");

    let mut db = Database::open()?;
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
