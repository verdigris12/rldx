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

    let mut app = ui::app::App::new(&db, &config)?;
    app.run()?;

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
    let stored = db.stored_items()?;

    for path in files {
        let mut state = vdir::compute_file_state(&path)?;
        let mut requires_index = force;

        if !requires_index {
            requires_index = match stored.get(&path) {
                Some(existing) => existing.sha1 != state.sha1 || existing.mtime != state.mtime,
                None => true,
            };
        }

        let parsed = vcard_io::parse_file(&path, config.phone_region.as_deref())?;
        let cards = parsed.cards;
        let changed = parsed.changed;

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

        if changed {
            state = vdir::compute_file_state(&path)?;
            requires_index = true;
        }

        if !requires_index {
            continue;
        }

        let card = cards.into_iter().next().unwrap();
        let record = indexer::build_record(&path, &card, &state, None)?;
        db.upsert(&record.item, &record.props)?;
    }

    db.remove_missing(&paths_set)?;
    Ok(())
}
