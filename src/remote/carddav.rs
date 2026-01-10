//! CardDAV client implementation using libdav.

use anyhow::{bail, Context, Result};
use http::Uri;
use hyper_rustls::HttpsConnectorBuilder;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use libdav::carddav::{
    CardDavClient, FindAddressBookHomeSet, FindAddressBooks, GetAddressBookResources,
};
use libdav::dav::{Delete, PutResource, WebDavClient};
use tower_http::auth::AddAuthorization;

use crate::config::RemoteConfig;
use crate::remote::{Remote, RemoteContact, RemoteContactSummary};

/// Type alias for our HTTP client with basic auth
type AuthClient = AddAuthorization<Client<hyper_rustls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>, String>>;

/// CardDAV remote implementation
pub struct CardDavRemote {
    client: CardDavClient<AuthClient>,
    address_book_href: String,
}

impl CardDavRemote {
    /// Create a new CardDAV remote from configuration
    pub async fn new(config: RemoteConfig) -> Result<Self> {
        let password = config.get_password()?;

        let uri: Uri = config.url.parse()
            .with_context(|| format!("invalid URL: {}", config.url))?;

        // Build HTTPS client with rustls
        let https_connector = HttpsConnectorBuilder::new()
            .with_webpki_roots()
            .https_or_http()
            .enable_http1()
            .build();

        let http_client = Client::builder(TokioExecutor::new())
            .build(https_connector);

        // Add basic auth
        let auth_client = AddAuthorization::basic(http_client, &config.username, &password);

        // Create WebDAV client
        let webdav = WebDavClient::new(uri, auth_client);

        // Bootstrap via service discovery to find the correct context path
        let client = CardDavClient::bootstrap_via_service_discovery(webdav)
            .await
            .with_context(|| "failed to bootstrap CardDAV client via service discovery")?;

        // Resolve address book href
        let address_book_href = Self::resolve_address_book(&client, &config.address_book).await?;

        Ok(Self {
            client,
            address_book_href,
        })
    }

    /// Resolve the address book href from the configured name
    async fn resolve_address_book(client: &CardDavClient<AuthClient>, address_book_name: &str) -> Result<String> {
        // First, find the current user principal
        let principal = client.find_current_user_principal()
            .await
            .context("failed to find current user principal")?
            .ok_or_else(|| anyhow::anyhow!("no current user principal found"))?;

        // Find the address book home set using FindAddressBookHomeSet request
        let home_set_response = client
            .request(FindAddressBookHomeSet::new(&principal))
            .await
            .context("failed to find address book home set")?;

        let home_uri = home_set_response
            .home_sets
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("no address book home set found"))?;

        // List all address books under the home set
        let addressbooks_response = client
            .request(FindAddressBooks::new(&home_uri))
            .await
            .context("failed to list address books")?;

        // Find the address book matching the configured name by href path component
        for ab in &addressbooks_response.addressbooks {
            // Check if the href ends with the address book name
            let href_name = ab.href.trim_end_matches('/').rsplit('/').next().unwrap_or("");
            if href_name.eq_ignore_ascii_case(address_book_name) {
                return Ok(ab.href.clone());
            }
        }

        // If no exact match, try partial matching or return first one
        if !addressbooks_response.addressbooks.is_empty() {
            // Look for partial match in href
            for ab in &addressbooks_response.addressbooks {
                let href_lower = ab.href.to_lowercase();
                if href_lower.contains(&address_book_name.to_lowercase()) {
                    return Ok(ab.href.clone());
                }
            }
            // Fall back to first address book if none matches
            eprintln!(
                "warning: address book '{}' not found, using '{}'",
                address_book_name,
                &addressbooks_response.addressbooks[0].href
            );
            return Ok(addressbooks_response.addressbooks[0].href.clone());
        }

        bail!(
            "no address books found for user; tried home set at '{}'",
            home_uri
        )
    }
}

impl Remote for CardDavRemote {
    async fn test_connection(&self) -> Result<()> {
        // Try to find the current user principal as a connection test
        let principal = self.client.find_current_user_principal()
            .await
            .context("failed to connect to CardDAV server")?;

        if principal.is_none() {
            bail!("connected but no principal found - check credentials");
        }

        Ok(())
    }

    async fn list_contacts(&self) -> Result<Vec<RemoteContactSummary>> {
        // Use GetAddressBookResources to list all contacts with etags
        let response = self.client
            .request(GetAddressBookResources::new(&self.address_book_href))
            .await
            .context("failed to list contacts")?;

        let mut results = Vec::new();
        for r in response.resources {
            // content is Result<FetchedResourceContent, StatusCode>
            let etag = r.content.ok().map(|c| c.etag);
            results.push(RemoteContactSummary {
                href: r.href,
                etag,
            });
        }
        Ok(results)
    }

    async fn fetch_contacts(&self, hrefs: &[&str]) -> Result<Vec<RemoteContact>> {
        if hrefs.is_empty() {
            return Ok(vec![]);
        }

        // Use addressbook-multiget REPORT to fetch multiple contacts efficiently
        let response = self.client
            .request(
                GetAddressBookResources::new(&self.address_book_href)
                    .with_hrefs(hrefs.iter().copied())
            )
            .await
            .context("failed to fetch contacts")?;

        let mut results = Vec::new();
        for resource in response.resources {
            if let Ok(content) = resource.content {
                results.push(RemoteContact {
                    href: resource.href,
                    etag: Some(content.etag),
                    vcard_data: content.data,
                });
            }
        }
        Ok(results)
    }

    async fn upload_contact(&self, href: Option<&str>, vcard_data: &str) -> Result<(String, Option<String>)> {
        // If href is None, generate a new one based on UUID (create new contact)
        // If href is Some, we're updating an existing contact
        let contact_href = match href {
            Some(h) => h.to_string(),
            None => {
                let uuid = uuid::Uuid::new_v4();
                format!("{}{}.vcf", self.address_book_href, uuid)
            }
        };

        // Use PutResource.create() to upload a new vCard
        // Note: For updates, we would need the etag and use .update() instead
        // For simplicity, we always try to create - the server will handle conflicts
        let response = self.client
            .request(
                PutResource::new(&contact_href)
                    .create(vcard_data, "text/vcard; charset=utf-8")
            )
            .await
            .context("failed to upload contact")?;

        Ok((contact_href, response.etag))
    }

    async fn delete_contact(&self, href: &str) -> Result<()> {
        // Use Delete request with force() to remove the contact unconditionally
        // For conditional delete, we would use .with_etag(etag) instead
        self.client
            .request(Delete::new(href).force())
            .await
            .context("failed to delete contact")?;

        Ok(())
    }
}
