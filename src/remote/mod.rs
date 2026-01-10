//! Remote server abstraction and CardDAV client implementation.
//!
//! This module provides:
//! - `Remote` trait for abstracting different remote server types
//! - `CardDavRemote` implementation using the libdav crate
//! - Types for representing remote contacts and their state

pub mod carddav;

use anyhow::Result;

/// A contact fetched from a remote server
#[derive(Debug, Clone)]
pub struct RemoteContact {
    /// The href (path) on the remote server
    pub href: String,
    /// The ETag for change detection
    pub etag: Option<String>,
    /// The vCard data as a string
    pub vcard_data: String,
}

/// Summary of a remote contact (without full vCard data)
#[derive(Debug, Clone)]
pub struct RemoteContactSummary {
    /// The href (path) on the remote server
    pub href: String,
    /// The ETag for change detection
    pub etag: Option<String>,
}

/// Trait for remote server implementations
#[allow(async_fn_in_trait)]
pub trait Remote {
    /// Test connection to the remote server
    async fn test_connection(&self) -> Result<()>;

    /// List all contacts in the configured address book (summaries only)
    async fn list_contacts(&self) -> Result<Vec<RemoteContactSummary>>;

    /// Fetch multiple contacts by href
    async fn fetch_contacts(&self, hrefs: &[&str]) -> Result<Vec<RemoteContact>>;

    /// Upload a contact to the remote
    /// Returns the href and new etag
    async fn upload_contact(&self, href: Option<&str>, vcard_data: &str) -> Result<(String, Option<String>)>;

    /// Delete a contact on the remote
    async fn delete_contact(&self, href: &str) -> Result<()>;
}
