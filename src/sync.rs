//! Sync engine for bidirectional CardDAV synchronization.
//!
//! This module orchestrates the synchronization between local vCard files
//! and remote CardDAV servers.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use indicatif::{ProgressBar, ProgressStyle};

use crate::config::{ConflictPreference, Config, RemoteConfig};
use crate::crypto::CryptoProvider;
use crate::db::{Database, SyncMetadata};
use crate::remote::Remote;
use crate::vdir;

/// Result of a sync operation
#[derive(Debug, Default)]
pub struct SyncResult {
    /// Number of contacts downloaded/updated from remote
    pub downloaded_count: usize,
    /// Number of contacts uploaded to remote
    pub uploaded_count: usize,
    /// Number of contacts deleted from remote
    pub deleted_remote_count: usize,
    /// Number of contacts deleted locally
    pub deleted_local_count: usize,
    /// Errors encountered during sync
    pub errors: Vec<SyncError>,
}

/// An error during sync
#[derive(Debug)]
pub struct SyncError {
    /// The contact path or href involved
    pub path: String,
    /// Description of the error
    pub message: String,
}

impl std::fmt::Display for SyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path, self.message)
    }
}

/// Sync engine for CardDAV synchronization
pub struct SyncEngine<'a> {
    config: &'a Config,
    remote_config: &'a RemoteConfig,
    db: &'a mut Database,
    provider: &'a dyn CryptoProvider,
    vdir: PathBuf,
    dry_run: bool,
    pull_only: bool,
}

impl<'a> SyncEngine<'a> {
    /// Create a new sync engine
    pub fn new(
        config: &'a Config,
        remote_config: &'a RemoteConfig,
        db: &'a mut Database,
        provider: &'a dyn CryptoProvider,
        dry_run: bool,
        pull_only: bool,
    ) -> Self {
        // Determine the local directory for this remote's contacts
        let vdir = match &remote_config.local_book {
            Some(book) => config.vdir.join(book),
            None => config.vdir.clone(),
        };

        Self {
            config,
            remote_config,
            db,
            provider,
            vdir,
            dry_run,
            pull_only,
        }
    }

    /// Run the sync operation
    pub async fn sync<R: Remote>(&mut self, remote: &R) -> Result<SyncResult> {
        let mut result = SyncResult::default();

        // Ensure local directory exists
        if !self.dry_run && !self.vdir.exists() {
            fs::create_dir_all(&self.vdir)
                .with_context(|| format!("failed to create directory: {}", self.vdir.display()))?;
        }

        // Phase 1: Pull changes from remote
        println!("Pulling changes from remote...");
        self.pull_changes(remote, &mut result).await?;

        // Phase 2: Push local changes (if not pull_only)
        if !self.pull_only {
            println!("Pushing local changes to remote...");
            self.push_changes(remote, &mut result).await?;
        }

        // Print summary
        self.print_summary(&result);

        Ok(result)
    }

    /// Pull changes from remote to local
    async fn pull_changes<R: Remote>(&mut self, remote: &R, result: &mut SyncResult) -> Result<()> {
        // Get list of all contacts on remote with their etags
        let remote_contacts = remote.list_contacts().await
            .context("failed to list remote contacts")?;

        // Get existing sync metadata for this remote
        let sync_metadata = self.db.get_sync_metadata_for_remote(&self.remote_config.name)?;
        let metadata_by_href: HashMap<String, SyncMetadata> = sync_metadata
            .into_iter()
            .map(|m| (m.remote_href.clone(), m))
            .collect();

        // Find contacts that need to be downloaded (new or changed)
        let mut to_download: Vec<String> = Vec::new();
        let mut remote_hrefs: HashMap<String, Option<String>> = HashMap::new();

        for contact in &remote_contacts {
            remote_hrefs.insert(contact.href.clone(), contact.etag.clone());

            if let Some(meta) = metadata_by_href.get(&contact.href) {
                // Check if etag has changed
                let etag_changed = match (&contact.etag, &meta.remote_etag) {
                    (Some(new_etag), Some(old_etag)) => new_etag != old_etag,
                    (Some(_), None) => true,
                    (None, Some(_)) => true,
                    (None, None) => false, // Can't detect changes without etags
                };

                if etag_changed {
                    to_download.push(contact.href.clone());
                }
            } else {
                // New contact not in our metadata
                to_download.push(contact.href.clone());
            }
        }

        // Download changed/new contacts
        if !to_download.is_empty() {
            let pb = self.create_progress_bar(to_download.len() as u64, "Downloading");

            // Fetch in batches for efficiency
            let batch_size = 50;
            for chunk in to_download.chunks(batch_size) {
                let hrefs: Vec<&str> = chunk.iter().map(|s| s.as_str()).collect();
                let contacts = remote.fetch_contacts(&hrefs).await
                    .context("failed to fetch contacts")?;

                for contact in contacts {
                    pb.inc(1);

                    // Determine if this is a new contact or update
                    let is_new = !metadata_by_href.contains_key(&contact.href);

                    if self.dry_run {
                        println!(
                            "[dry-run] Would {} contact: {}",
                            if is_new { "download" } else { "update" },
                            &contact.href
                        );
                        continue;
                    }

                    // Save the contact locally
                    match self.save_contact_locally(&contact.href, &contact.vcard_data, &contact.etag).await {
                        Ok(_local_path) => {
                            result.downloaded_count += 1;
                        }
                        Err(e) => {
                            result.errors.push(SyncError {
                                path: contact.href.clone(),
                                message: format!("failed to save: {}", e),
                            });
                        }
                    }
                }
            }

            pb.finish_with_message("Download complete");
        }

        // Find contacts that were deleted on remote
        for (href, meta) in &metadata_by_href {
            if !remote_hrefs.contains_key(href) {
                // Contact was deleted on remote
                if self.dry_run {
                    println!("[dry-run] Would delete local contact: {}", meta.contact_path.display());
                    continue;
                }

                // Handle conflict: local might have been modified
                if meta.local_modified {
                    let preference = self.get_conflict_preference();
                    match preference {
                        ConflictPreference::Ours => {
                            // Keep local, will be re-uploaded in push phase
                            continue;
                        }
                        ConflictPreference::Theirs => {
                            // Delete local
                        }
                    }
                }

                // Delete local file
                if meta.contact_path.exists() {
                    if let Err(e) = fs::remove_file(&meta.contact_path) {
                        result.errors.push(SyncError {
                            path: meta.contact_path.display().to_string(),
                            message: format!("failed to delete: {}", e),
                        });
                        continue;
                    }
                }

                // Remove from database
                self.db.delete_sync_metadata(&meta.contact_path, &self.remote_config.name)?;
                self.db.delete_items_by_paths([meta.contact_path.clone()])?;

                result.deleted_local_count += 1;
            }
        }

        Ok(())
    }

    /// Push local changes to remote
    async fn push_changes<R: Remote>(&mut self, remote: &R, result: &mut SyncResult) -> Result<()> {
        // Get all local vCard files
        let local_files = vdir::list_vcf_files(&self.vdir)?;

        // Get existing sync metadata for this remote
        let sync_metadata = self.db.get_sync_metadata_for_remote(&self.remote_config.name)?;
        let metadata_by_path: HashMap<PathBuf, SyncMetadata> = sync_metadata
            .into_iter()
            .map(|m| (m.contact_path.clone(), m))
            .collect();

        // Find local files that need to be uploaded
        let mut to_upload: Vec<(PathBuf, Option<String>)> = Vec::new(); // (path, href if update)

        for path in &local_files {
            if let Some(meta) = metadata_by_path.get(path) {
                // Check if file was modified since last sync
                if meta.local_modified || self.file_modified_since(path, meta.last_synced)? {
                    // File was modified locally, need to upload
                    to_upload.push((path.clone(), Some(meta.remote_href.clone())));
                }
            } else {
                // New local file not synced yet
                to_upload.push((path.clone(), None));
            }
        }

        // Upload modified/new contacts
        if !to_upload.is_empty() {
            let pb = self.create_progress_bar(to_upload.len() as u64, "Uploading");

            for (path, href) in to_upload {
                pb.inc(1);

                if self.dry_run {
                    println!(
                        "[dry-run] Would {} contact: {}",
                        if href.is_some() { "update" } else { "upload new" },
                        path.display()
                    );
                    continue;
                }

                // Read the vCard file
                let vcard_data = match vdir::read_vcf_file(&path, self.provider) {
                    Ok(data) => data,
                    Err(e) => {
                        result.errors.push(SyncError {
                            path: path.display().to_string(),
                            message: format!("failed to read: {}", e),
                        });
                        continue;
                    }
                };

                // Upload to remote
                match remote.upload_contact(href.as_deref(), &vcard_data).await {
                    Ok((new_href, new_etag)) => {
                        // Update sync metadata
                        let meta = SyncMetadata {
                            contact_path: path.clone(),
                            remote_name: self.remote_config.name.clone(),
                            remote_href: new_href.clone(),
                            remote_etag: new_etag.clone(),
                            last_synced: Some(current_timestamp()),
                            local_modified: false,
                        };
                        self.db.upsert_sync_metadata(&meta)?;

                        result.uploaded_count += 1;
                    }
                    Err(e) => {
                        result.errors.push(SyncError {
                            path: path.display().to_string(),
                            message: format!("failed to upload: {}", e),
                        });
                    }
                }
            }

            pb.finish_with_message("Upload complete");
        }

        // Find local files that were deleted (in metadata but file doesn't exist)
        for (path, meta) in &metadata_by_path {
            if !path.exists() {
                if self.dry_run {
                    println!("[dry-run] Would delete remote contact: {}", meta.remote_href);
                    continue;
                }

                // Delete from remote
                if let Err(e) = remote.delete_contact(&meta.remote_href).await {
                    result.errors.push(SyncError {
                        path: meta.remote_href.clone(),
                        message: format!("failed to delete from remote: {}", e),
                    });
                    continue;
                }

                // Remove sync metadata
                self.db.delete_sync_metadata(path, &self.remote_config.name)?;

                result.deleted_remote_count += 1;
            }
        }

        Ok(())
    }

    /// Save a contact locally and update sync metadata
    async fn save_contact_locally(
        &mut self,
        href: &str,
        vcard_data: &str,
        etag: &Option<String>,
    ) -> Result<PathBuf> {
        // Generate a filename based on UUID (to avoid metadata leakage from remote filenames)
        // Try to extract UID from vCard, fall back to UUID
        let uid = extract_uid_from_vcard(vcard_data)
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        let filename = format!("{}.vcf", sanitize_filename(&uid));
        let local_path = self.vdir.join(&filename);

        // Check if we already have this contact at a different path (by href)
        if let Some(existing_meta) = self.db.get_sync_metadata_for_remote(&self.remote_config.name)?
            .into_iter()
            .find(|m| m.remote_href == href)
        {
            // Use the existing local path
            let existing_path = existing_meta.contact_path;

            // Write the file
            vdir::write_vcf_file(&existing_path, vcard_data.as_bytes(), self.provider)?;

            // Update sync metadata
            let meta = SyncMetadata {
                contact_path: existing_path.clone(),
                remote_name: self.remote_config.name.clone(),
                remote_href: href.to_string(),
                remote_etag: etag.clone(),
                last_synced: Some(current_timestamp()),
                local_modified: false,
            };
            self.db.upsert_sync_metadata(&meta)?;

            return Ok(existing_path);
        }

        // Write the vCard file (encrypted)
        vdir::write_vcf_file(&local_path, vcard_data.as_bytes(), self.provider)?;

        // Update sync metadata
        let meta = SyncMetadata {
            contact_path: local_path.clone(),
            remote_name: self.remote_config.name.clone(),
            remote_href: href.to_string(),
            remote_etag: etag.clone(),
            last_synced: Some(current_timestamp()),
            local_modified: false,
        };
        self.db.upsert_sync_metadata(&meta)?;

        Ok(local_path)
    }

    /// Check if file was modified since a given timestamp
    fn file_modified_since(&self, path: &Path, timestamp: Option<i64>) -> Result<bool> {
        let Some(last_synced) = timestamp else {
            return Ok(true); // No timestamp means never synced
        };

        let metadata = fs::metadata(path)
            .with_context(|| format!("failed to stat file: {}", path.display()))?;

        let mtime = metadata.modified()
            .with_context(|| "failed to get mtime")?
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        Ok(mtime > last_synced)
    }

    /// Get the conflict preference for this remote
    fn get_conflict_preference(&self) -> ConflictPreference {
        self.remote_config
            .conflict_prefer
            .unwrap_or(self.config.sync.conflict_prefer)
    }

    /// Create a progress bar
    fn create_progress_bar(&self, total: u64, message: &str) -> ProgressBar {
        let pb = ProgressBar::new(total);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{msg} [{bar:40.cyan/blue}] {pos}/{len} ({eta})")
                .unwrap_or_else(|_| ProgressStyle::default_bar())
                .progress_chars("=>-"),
        );
        pb.set_message(message.to_string());
        pb
    }

    /// Print sync summary
    fn print_summary(&self, result: &SyncResult) {
        println!();
        println!("Sync completed:");
        println!("  Downloaded: {} contact(s)", result.downloaded_count);
        println!("  Uploaded:   {} contact(s)", result.uploaded_count);
        println!("  Deleted (local):  {} contact(s)", result.deleted_local_count);
        println!("  Deleted (remote): {} contact(s)", result.deleted_remote_count);

        if !result.errors.is_empty() {
            println!("  Errors:     {} error(s)", result.errors.len());
            for err in &result.errors {
                eprintln!("    - {}", err);
            }
        }
    }
}

/// Get current timestamp as Unix epoch seconds
fn current_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// Extract UID property from vCard data
fn extract_uid_from_vcard(vcard_data: &str) -> Option<String> {
    for line in vcard_data.lines() {
        let line = line.trim();
        if line.to_uppercase().starts_with("UID:") {
            return Some(line[4..].trim().to_string());
        }
        // Handle UID with parameters like UID;VALUE=text:...
        if line.to_uppercase().starts_with("UID;") {
            if let Some(pos) = line.find(':') {
                return Some(line[pos + 1..].trim().to_string());
            }
        }
    }
    None
}

/// Sanitize a string for use as a filename
fn sanitize_filename(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => c,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_uid_from_vcard() {
        let vcard = r#"BEGIN:VCARD
VERSION:4.0
UID:urn:uuid:f81d4fae-7dec-11d0-a765-00a0c91e6bf6
FN:John Doe
END:VCARD"#;
        assert_eq!(
            extract_uid_from_vcard(vcard),
            Some("urn:uuid:f81d4fae-7dec-11d0-a765-00a0c91e6bf6".to_string())
        );
    }

    #[test]
    fn test_extract_uid_with_params() {
        let vcard = r#"BEGIN:VCARD
VERSION:4.0
UID;VALUE=text:custom-uid-123
FN:Jane Doe
END:VCARD"#;
        assert_eq!(
            extract_uid_from_vcard(vcard),
            Some("custom-uid-123".to_string())
        );
    }

    #[test]
    fn test_sanitize_filename() {
        assert_eq!(sanitize_filename("normal"), "normal");
        assert_eq!(sanitize_filename("with/slash"), "with_slash");
        assert_eq!(sanitize_filename("a:b*c?d"), "a_b_c_d");
    }
}
