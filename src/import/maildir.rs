use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use mailparse::{addrparse, parse_mail, MailAddr, MailHeader, MailHeaderMap};
use rayon::prelude::*;
use strsim::jaro_winkler;

use vcard4::property::TextProperty;
use vcard4::Vcard;

use super::simhash_index::{NameSource, SimHashIndex};
use crate::config::Config;
use crate::crypto::CryptoProvider;
use crate::db::{compute_simhash, Database};
use crate::search;
use crate::vcard_io;
use crate::vdir;

/// Result of maildir import operation
pub struct ImportResult {
    pub imported: usize,
    pub merged: Vec<MergeInfo>,
    pub skipped: usize,
}

/// Information about a merged contact
pub struct MergeInfo {
    pub email: String,
    pub name: String,
    pub merged_into: String,
    pub score: f64,
}

/// Extracted contact from email headers
#[derive(Clone)]
struct ExtractedContact {
    email: String,
    primary_name: String,
    aliases: HashSet<String>,
    from_header: bool, // true if primary_name came from From header
}

/// Chunk size for processing emails (balances memory vs parallelism)
const CHUNK_SIZE: usize = 10000;

/// Import contacts from a maildir directory
pub fn import_maildir(
    input: &Path,
    config: &Config,
    book: Option<&str>,
    automerge_threshold: Option<f64>,
    threads: Option<usize>,
    db: &mut Database,
    provider: &dyn CryptoProvider,
) -> Result<ImportResult> {
    // Configure thread pool if specified
    if let Some(num_threads) = threads {
        rayon::ThreadPoolBuilder::new()
            .num_threads(num_threads)
            .build_global()
            .ok(); // Ignore error if pool already initialized
    }

    // Phase 1: Collect all mail file paths
    eprintln!("Scanning maildir for email files...");
    let mail_files = collect_all_mail_files(input)?;
    
    if mail_files.is_empty() {
        return Ok(ImportResult {
            imported: 0,
            merged: Vec::new(),
            skipped: 0,
        });
    }

    eprintln!("Found {} email files", mail_files.len());

    // Phase 2: Parse emails in parallel (chunked for memory efficiency)
    let contacts = parse_emails_parallel(&mail_files)?;

    if contacts.is_empty() {
        return Ok(ImportResult {
            imported: 0,
            merged: Vec::new(),
            skipped: 0,
        });
    }

    eprintln!("Extracted {} unique contacts", contacts.len());

    // Phase 3: Import contacts (sequential - involves file I/O and DB)
    import_contacts(contacts, config, book, automerge_threshold, db, provider)
}

/// Collect all mail file paths from maildir structure
fn collect_all_mail_files(path: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_maildir_files_recursive(path, &mut files)?;
    Ok(files)
}

/// Recursively collect mail files from maildir directories
fn collect_maildir_files_recursive(path: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    // Check if this directory is a maildir (has cur or new)
    let cur_path = path.join("cur");
    let new_path = path.join("new");

    if cur_path.exists() {
        collect_files_from_dir(&cur_path, files)?;
    }
    if new_path.exists() {
        collect_files_from_dir(&new_path, files)?;
    }

    // Recurse into subdirectories
    let entries = match fs::read_dir(path) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };

    for entry in entries.flatten() {
        if !entry.path().is_dir() {
            continue;
        }

        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        // Skip cur/new/tmp directories
        if name_str == "cur" || name_str == "new" || name_str == "tmp" {
            continue;
        }

        collect_maildir_files_recursive(&entry.path(), files)?;
    }

    Ok(())
}

/// Collect regular files from a directory
fn collect_files_from_dir(dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            files.push(path);
        }
    }

    Ok(())
}

/// Parse emails in parallel, processing in chunks for memory efficiency
fn parse_emails_parallel(mail_files: &[PathBuf]) -> Result<HashMap<String, ExtractedContact>> {
    let total = mail_files.len();
    
    // Create progress bar
    let pb = ProgressBar::new(total as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({percent}%) {msg}")
            .unwrap()
            .progress_chars("#>-"),
    );
    pb.set_message("Parsing emails...");

    // Process in chunks to limit memory usage
    let contacts: Mutex<HashMap<String, ExtractedContact>> = Mutex::new(HashMap::new());

    for chunk in mail_files.chunks(CHUNK_SIZE) {
        // Process chunk in parallel
        let chunk_contacts: Vec<_> = chunk
            .par_iter()
            .filter_map(|path| {
                pb.inc(1);
                process_single_email(path)
            })
            .flatten()
            .collect();

        // Merge chunk results into main map (sequential)
        let mut map = contacts.lock().unwrap();
        for (email, name, is_from) in chunk_contacts {
            merge_contact_entry(&mut map, email, name, is_from);
        }
    }

    pb.finish_with_message("Done parsing emails");

    Ok(contacts.into_inner().unwrap())
}

/// Process a single email file, returns extracted (email, name, is_from) tuples
fn process_single_email(path: &PathBuf) -> Option<Vec<(String, String, bool)>> {
    let data = fs::read(path).ok()?;
    let parsed = parse_mail(&data).ok()?;

    let mut results = Vec::new();

    // Extract From header (highest priority)
    extract_addresses_to_vec(&parsed.headers[..], "From", &mut results, true);

    // Extract other headers
    for header in ["To", "Cc", "Reply-To"] {
        extract_addresses_to_vec(&parsed.headers[..], header, &mut results, false);
    }

    if results.is_empty() {
        None
    } else {
        Some(results)
    }
}

/// Extract addresses from header into a vector of (email, name, is_from) tuples
fn extract_addresses_to_vec(
    headers: &[MailHeader],
    header_name: &str,
    results: &mut Vec<(String, String, bool)>,
    is_from: bool,
) {
    let Some(value) = headers.get_first_value(header_name) else {
        return;
    };

    let Ok(addrs) = addrparse(&value) else {
        return;
    };

    for addr in addrs.iter() {
        match addr {
            MailAddr::Single(info) => {
                let name = clean_name(info.display_name.as_deref().unwrap_or(""));
                let email = info.addr.trim().to_lowercase();
                if is_valid_contact(&email, &name) {
                    results.push((email, name, is_from));
                }
            }
            MailAddr::Group(group) => {
                for member in &group.addrs {
                    let name = clean_name(member.display_name.as_deref().unwrap_or(""));
                    let email = member.addr.trim().to_lowercase();
                    if is_valid_contact(&email, &name) {
                        results.push((email, name, is_from));
                    }
                }
            }
        }
    }
}

/// Clean up a name by stripping surrounding quotes and extra whitespace
fn clean_name(name: &str) -> String {
    let trimmed = name.trim();
    // Strip surrounding quotes (single or double)
    let stripped = trimmed
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| {
            trimmed
                .strip_prefix('\'')
                .and_then(|s| s.strip_suffix('\''))
        })
        .unwrap_or(trimmed);
    stripped.trim().to_string()
}

/// Check if a contact is valid for import (basic validation only)
fn is_valid_contact(email: &str, name: &str) -> bool {
    if email.is_empty() || name.is_empty() {
        return false;
    }

    let email_lower = email.to_lowercase();

    // Name must be meaningful
    name.to_lowercase() != email_lower && !name.contains('@') && name.len() >= 2
}

/// Merge a contact entry into the map
fn merge_contact_entry(
    map: &mut HashMap<String, ExtractedContact>,
    email: String,
    name: String,
    is_from: bool,
) {
    match map.get_mut(&email) {
        Some(existing) => {
            if is_from && !existing.from_header && name != existing.primary_name {
                // From header takes priority - demote current primary to alias
                existing.aliases.insert(existing.primary_name.clone());
                existing.primary_name = name;
                existing.from_header = true;
            } else if name != existing.primary_name {
                // Add as alias
                existing.aliases.insert(name);
            }
        }
        None => {
            map.insert(
                email.clone(),
                ExtractedContact {
                    email,
                    primary_name: name,
                    aliases: HashSet::new(),
                    from_header: is_from,
                },
            );
        }
    }
}

/// Import extracted contacts into vdir
fn import_contacts(
    contacts: HashMap<String, ExtractedContact>,
    config: &Config,
    book: Option<&str>,
    automerge_threshold: Option<f64>,
    db: &mut Database,
    provider: &dyn CryptoProvider,
) -> Result<ImportResult> {
    let target_dir = match book {
        Some(name) => config.vdir.join(name),
        None => config.vdir.clone(),
    };

    fs::create_dir_all(&target_dir).with_context(|| {
        format!(
            "failed to ensure target address book directory {}",
            target_dir.display()
        )
    })?;

    let mut used_names = vdir::existing_stems(&target_dir)?;
    let mut imported = 0usize;
    let mut merged = Vec::new();
    let mut skipped = 0usize;

    // Build SimHash index for automerge if enabled
    let simhash_index = if automerge_threshold.is_some() {
        let existing_simhashes = db.list_all_simhashes()?;
        Some(SimHashIndex::new(existing_simhashes))
    } else {
        None
    };
    let simhash_threshold = config.maildir_import.simhash_threshold;

    // Progress bar for import phase
    let pb = ProgressBar::new(contacts.len() as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({percent}%) {msg}")
            .unwrap()
            .progress_chars("#>-"),
    );
    pb.set_message("Importing contacts...");

    for contact in contacts.values() {
        pb.inc(1);

        // Skip if email matches configured filter patterns
        if config.maildir_import.should_skip_email(&contact.email) {
            skipped += 1;
            continue;
        }

        // Skip if email already exists in database
        if db.email_exists(&contact.email)? {
            skipped += 1;
            continue;
        }

        // Try to find merge candidate if automerge is enabled
        if let Some(threshold) = automerge_threshold {
            if let Some(ref index) = simhash_index {
                // Collect all names to check: primary name + aliases
                let aliases_vec: Vec<&str> = contact.aliases.iter().map(|s| s.as_str()).collect();

                if let Some((path, display_fn, score)) = find_merge_candidate(
                    index,
                    &contact.primary_name,
                    &aliases_vec,
                    threshold,
                    simhash_threshold,
                    &config.maildir_import,
                ) {
                    // Merge into existing contact
                    if merge_into_existing(
                        &path,
                        &contact.email,
                        &contact.aliases,
                        config.phone_region.as_deref(),
                        provider,
                    )? {
                        merged.push(MergeInfo {
                            email: contact.email.clone(),
                            name: contact.primary_name.clone(),
                            merged_into: display_fn,
                            score,
                        });
                        continue;
                    }
                }
            }
        }

        // Create new vCard
        match create_vcard(contact, config.phone_region.as_deref()) {
            Ok(mut card) => {
                let uuid = vcard_io::ensure_uuid_uid(&mut card)?;
                vcard_io::touch_rev(&mut card);

                let filename = vdir::select_filename(&uuid, &mut used_names, None);
                let path = vdir::vcf_target_path(&target_dir, &filename, provider.encryption_type());
                let bytes = vcard_io::card_to_bytes(&card);
                let encrypted = provider.encrypt(&bytes)
                    .with_context(|| format!("failed to encrypt vCard for {}", path.display()))?;
                vdir::write_atomic(&path, &encrypted)?;
                imported += 1;
            }
            Err(err) => {
                eprintln!(
                    "warning: skipping contact <{}>, conversion failed: {err}",
                    contact.email
                );
                skipped += 1;
            }
        }
    }

    pb.finish_with_message("Done importing contacts");

    Ok(ImportResult {
        imported,
        merged,
        skipped,
    })
}

/// Match result with source information for preferencing
struct MatchCandidate {
    path: PathBuf,
    display_fn: String,
    score: f64,
    source: NameSource,
}

/// Find a merge candidate using SimHash BK-tree for fast lookup, then Jaro-Winkler verification.
/// Checks both the primary name and aliases against existing FNs and nicknames.
/// Prefers FN matches over nickname matches.
fn find_merge_candidate(
    index: &SimHashIndex,
    primary_name: &str,
    aliases: &[&str],
    jw_threshold: f64,
    simhash_threshold: u32,
    import_config: &crate::config::MaildirImportConfig,
) -> Option<(PathBuf, String, f64)> {
    let mut all_candidates: Vec<MatchCandidate> = Vec::new();

    // Check primary name
    if import_config.is_valid_fn_for_merge(primary_name) {
        collect_candidates(
            index,
            primary_name,
            jw_threshold,
            simhash_threshold,
            &mut all_candidates,
        );
    }

    // Check aliases
    for alias in aliases {
        if import_config.is_valid_nickname_for_merge(alias) {
            collect_candidates(
                index,
                alias,
                jw_threshold,
                simhash_threshold,
                &mut all_candidates,
            );
        }
    }

    if all_candidates.is_empty() {
        return None;
    }

    // Sort candidates: FN matches first, then by score descending
    all_candidates.sort_by(|a, b| {
        // FN matches are preferred over nickname matches
        match (&a.source, &b.source) {
            (NameSource::FN, NameSource::Nickname) => std::cmp::Ordering::Less,
            (NameSource::Nickname, NameSource::FN) => std::cmp::Ordering::Greater,
            _ => b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal),
        }
    });

    // Return the best match
    all_candidates
        .into_iter()
        .next()
        .map(|c| (c.path, c.display_fn, c.score))
}

/// Collect matching candidates for a single name
fn collect_candidates(
    index: &SimHashIndex,
    name: &str,
    jw_threshold: f64,
    simhash_threshold: u32,
    results: &mut Vec<MatchCandidate>,
) {
    let name_norm = search::normalize(name);
    let name_simhash = compute_simhash(&name_norm);

    // Find candidates within Hamming distance threshold using BK-tree
    let candidates = index.find_candidates(name_simhash, simhash_threshold);

    for entry in candidates {
        let score = jaro_winkler(&name_norm, &entry.matched_norm);

        if score >= jw_threshold {
            // Avoid duplicate paths with lower scores
            let dominated = results.iter().any(|r| {
                r.path == entry.path
                    && (r.source == NameSource::FN || entry.source == NameSource::Nickname)
                    && r.score >= score
            });

            if !dominated {
                // Remove any dominated entries for this path
                results.retain(|r| {
                    r.path != entry.path
                        || (r.source == NameSource::FN && entry.source == NameSource::Nickname)
                        || r.score > score
                });

                results.push(MatchCandidate {
                    path: entry.path.clone(),
                    display_fn: entry.display_fn.clone(),
                    score,
                    source: entry.source,
                });
            }
        }
    }
}

/// Merge email and aliases into an existing vCard
fn merge_into_existing(
    path: &Path,
    email: &str,
    aliases: &HashSet<String>,
    default_region: Option<&str>,
    provider: &dyn CryptoProvider,
) -> Result<bool> {
    let parsed = vcard_io::parse_file(path, default_region, provider)?;
    let Some(mut card) = parsed.cards.into_iter().next() else {
        return Ok(false);
    };

    let mut changed = false;

    // Add email if not present
    let email_lower = email.to_lowercase();
    let has_email = card.email.iter().any(|e| e.value.to_lowercase() == email_lower);

    if !has_email {
        card.email.push(TextProperty {
            group: None,
            value: email.to_string(),
            parameters: None,
        });
        changed = true;
    }

    // Add aliases as nicknames if not present
    for alias in aliases {
        let alias_lower = alias.to_lowercase();
        let has_alias = card.nickname.iter().any(|n| n.value.to_lowercase() == alias_lower);

        if !has_alias {
            card.nickname.push(TextProperty {
                group: None,
                value: alias.clone(),
                parameters: None,
            });
            changed = true;
        }
    }

    if changed {
        vcard_io::touch_rev(&mut card);
        let bytes = vcard_io::card_to_bytes(&card);
        let encrypted = provider.encrypt(&bytes)
            .with_context(|| format!("failed to encrypt vCard for {}", path.display()))?;
        vdir::write_atomic(path, &encrypted)?;
    }

    Ok(changed)
}

/// Create a new vCard from extracted contact
fn create_vcard(contact: &ExtractedContact, default_region: Option<&str>) -> Result<Vcard> {
    let mut lines = vec![
        "BEGIN:VCARD".to_string(),
        "VERSION:4.0".to_string(),
        format!("FN:{}", escape_vcard_value(&contact.primary_name)),
        format!("EMAIL:{}", contact.email),
    ];

    // Add aliases as nicknames
    for alias in &contact.aliases {
        lines.push(format!("NICKNAME:{}", escape_vcard_value(alias)));
    }

    lines.push("END:VCARD".to_string());

    let vcard_str = lines.join("\r\n");
    let parsed = vcard_io::parse_str(&vcard_str, default_region)?;

    parsed
        .cards
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("failed to create vCard"))
}

/// Escape special characters in vCard values
fn escape_vcard_value(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace(',', "\\,")
        .replace(';', "\\;")
        .replace('\n', "\\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clean_name_strips_double_quotes() {
        assert_eq!(clean_name("\"John Smith\""), "John Smith");
        assert_eq!(clean_name("  \"John Smith\"  "), "John Smith");
    }

    #[test]
    fn test_clean_name_strips_single_quotes() {
        assert_eq!(clean_name("'John Smith'"), "John Smith");
        assert_eq!(clean_name("  'John Smith'  "), "John Smith");
    }

    #[test]
    fn test_clean_name_no_quotes() {
        assert_eq!(clean_name("John Smith"), "John Smith");
        assert_eq!(clean_name("  John Smith  "), "John Smith");
    }

    #[test]
    fn test_clean_name_mismatched_quotes() {
        // Mismatched quotes should not be stripped
        assert_eq!(clean_name("\"John Smith'"), "\"John Smith'");
        assert_eq!(clean_name("'John Smith\""), "'John Smith\"");
    }

    #[test]
    fn test_clean_name_partial_quotes() {
        // Only leading or trailing quote should not be stripped
        assert_eq!(clean_name("\"John Smith"), "\"John Smith");
        assert_eq!(clean_name("John Smith\""), "John Smith\"");
    }
}
