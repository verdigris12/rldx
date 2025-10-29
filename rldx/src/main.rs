mod config;
mod db;
mod indexer;
mod photo;
mod search;
mod vcard_io;
mod vdir;
mod ui;

use std::collections::HashSet;

use anyhow::Result;
use clap::Parser;

use config::Config;
use db::Database;

#[derive(Parser, Debug)]
#[command(name = "rldx")]
struct Cli {
    #[arg(long, default_value_t = false)]
    reindex: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = config::load()?;

    println!("Loaded configuration from {}", config.config_path.display());

    let normalize_report = vdir::normalize(&config.vdir)?;
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

fn reindex(db: &mut Database, config: &Config, force: bool) -> Result<()> {
    let files = vdir::list_vcf_files(&config.vdir)?;
    let paths_set: HashSet<_> = files.iter().cloned().collect();
    let stored = db.stored_items()?;

    for path in files {
        let state = vdir::compute_file_state(&path)?;
        let needs_update = match stored.get(&path) {
            Some(existing) if !force && existing.sha1 == state.sha1 && existing.mtime == state.mtime => false,
            _ => true,
        };

        if !needs_update {
            continue;
        }

        let cards = vcard_io::parse_file(&path)?;
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
        let card = cards.into_iter().next().unwrap();
        let record = indexer::build_record(&path, &card, &state, None)?;
        db.upsert(&record.item, &record.props)?;
    }

    db.remove_missing(&paths_set)?;
    Ok(())
}
