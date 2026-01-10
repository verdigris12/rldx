//! Encryption providers for vCard and database encryption.
//!
//! Supports two backends:
//! - GPG: Uses gpg-agent for key management, stores files as .vcf.gpg
//! - Age: Modern encryption, stores files as .vcf.age

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{anyhow, bail, Context, Result};

use crate::config::{EncryptionConfig, EncryptionType};

// =============================================================================
// CryptoProvider Trait
// =============================================================================

/// Trait for encryption/decryption providers
#[allow(dead_code)] // Used once full encryption integration is complete
pub trait CryptoProvider: Send + Sync {
    /// Encrypt plaintext data, returning ciphertext
    fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>>;

    /// Decrypt ciphertext, returning plaintext
    fn decrypt(&self, ciphertext: &[u8]) -> Result<Vec<u8>>;

    /// Derive a deterministic key for SQLCipher database encryption
    /// The key should be derived from the encryption key material
    fn derive_db_key(&self) -> Result<String>;

    /// Get the file extension for encrypted files (e.g., "vcf.gpg")
    fn file_extension(&self) -> &'static str;

    /// Get the encryption type
    fn encryption_type(&self) -> EncryptionType;
}

// =============================================================================
// GPG Provider
// =============================================================================

/// GPG encryption provider using gpg command-line tool
#[allow(dead_code)] // Used once full encryption integration is complete
pub struct GpgProvider {
    key_id: String,
}

#[allow(dead_code)] // Used once full encryption integration is complete
impl GpgProvider {
    pub fn new(key_id: String) -> Result<Self> {
        // Verify GPG is available and key exists
        let output = Command::new("gpg")
            .args(["--list-keys", &key_id])
            .output()
            .context("failed to execute gpg - is GPG installed?")?;

        if !output.status.success() {
            bail!(
                "GPG key '{}' not found. Make sure the key is imported.\nGPG error: {}",
                key_id,
                String::from_utf8_lossy(&output.stderr)
            );
        }

        Ok(Self { key_id })
    }

    /// Get the key fingerprint for deterministic operations
    fn get_key_fingerprint(&self) -> Result<String> {
        let output = Command::new("gpg")
            .args([
                "--with-colons",
                "--fingerprint",
                &self.key_id,
            ])
            .output()
            .context("failed to get GPG key fingerprint")?;

        if !output.status.success() {
            bail!("failed to get fingerprint for key '{}'", self.key_id);
        }

        // Parse fingerprint from colon-delimited output
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            if line.starts_with("fpr:") {
                let parts: Vec<&str> = line.split(':').collect();
                if parts.len() > 9 {
                    return Ok(parts[9].to_string());
                }
            }
        }

        bail!("could not parse fingerprint for key '{}'", self.key_id)
    }
}

impl CryptoProvider for GpgProvider {
    fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>> {
        let mut child = Command::new("gpg")
            .args([
                "--encrypt",
                "--armor",
                "--recipient",
                &self.key_id,
                "--batch",
                "--yes",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("failed to spawn gpg for encryption")?;

        {
            let stdin = child.stdin.as_mut().context("failed to open gpg stdin")?;
            stdin
                .write_all(plaintext)
                .context("failed to write to gpg stdin")?;
        }

        let output = child.wait_with_output().context("failed to wait for gpg")?;

        if !output.status.success() {
            bail!(
                "GPG encryption failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        Ok(output.stdout)
    }

    fn decrypt(&self, ciphertext: &[u8]) -> Result<Vec<u8>> {
        let mut child = Command::new("gpg")
            .args(["--decrypt", "--batch", "--yes"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("failed to spawn gpg for decryption")?;

        {
            let stdin = child.stdin.as_mut().context("failed to open gpg stdin")?;
            stdin
                .write_all(ciphertext)
                .context("failed to write to gpg stdin")?;
        }

        let output = child.wait_with_output().context("failed to wait for gpg")?;

        if !output.status.success() {
            bail!(
                "GPG decryption failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        Ok(output.stdout)
    }

    fn derive_db_key(&self) -> Result<String> {
        // Use key fingerprint + fixed salt to derive a deterministic DB key
        // We use GPG to encrypt a known message and hash the result
        let fingerprint = self.get_key_fingerprint()?;

        // Create a deterministic key by hashing fingerprint with a salt
        // This ensures the same GPG key always produces the same DB key
        use sha1::{Digest, Sha1};
        let mut hasher = Sha1::new();
        hasher.update(b"rldx-sqlcipher-key-v1:");
        hasher.update(fingerprint.as_bytes());
        let hash = hasher.finalize();

        // Convert to hex string for SQLCipher PRAGMA key
        Ok(format!("x'{}'", hex::encode(hash)))
    }

    fn file_extension(&self) -> &'static str {
        "vcf.gpg"
    }

    fn encryption_type(&self) -> EncryptionType {
        EncryptionType::Gpg
    }
}

// =============================================================================
// Age Provider
// =============================================================================

/// Age encryption provider using the age library
#[allow(dead_code)] // Used once full encryption integration is complete
pub struct AgeProvider {
    identity_path: PathBuf,
    recipient: String,
}

#[allow(dead_code)] // Used once full encryption integration is complete
impl AgeProvider {
    pub fn new(identity_path: PathBuf, recipient: String) -> Result<Self> {
        // Validate identity file exists
        if !identity_path.exists() {
            bail!(
                "age identity file not found: {}",
                identity_path.display()
            );
        }

        // Validate recipient format
        if !recipient.starts_with("age1") {
            bail!(
                "invalid age recipient '{}' - must start with 'age1'",
                recipient
            );
        }

        Ok(Self {
            identity_path,
            recipient,
        })
    }

    /// Read the identity file and parse identities
    fn read_identities(&self) -> Result<Vec<age::x25519::Identity>> {
        use std::fs;
        let content = fs::read_to_string(&self.identity_path)
            .with_context(|| format!("failed to read identity file: {}", self.identity_path.display()))?;

        let mut identities = Vec::new();
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            // Parse X25519 identity
            if line.starts_with("AGE-SECRET-KEY-") {
                let identity: age::x25519::Identity = line
                    .parse()
                    .map_err(|e| anyhow!("failed to parse age identity: {}", e))?;
                identities.push(identity);
            }
        }

        if identities.is_empty() {
            bail!(
                "no valid age identities found in {}",
                self.identity_path.display()
            );
        }

        Ok(identities)
    }

    /// Parse recipient public key
    fn parse_recipient(&self) -> Result<age::x25519::Recipient> {
        self.recipient
            .parse()
            .map_err(|e| anyhow!("failed to parse age recipient '{}': {}", self.recipient, e))
    }
}

impl CryptoProvider for AgeProvider {
    fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>> {
        let recipient = self.parse_recipient()?;
        let recipients: Vec<&dyn age::Recipient> = vec![&recipient];
        let encryptor = age::Encryptor::with_recipients(recipients.into_iter())
            .map_err(|e| anyhow!("failed to create encryptor: {}", e))?;

        let mut encrypted = Vec::new();
        {
            // Use armored output for text-based storage
            let armored_writer = age::armor::ArmoredWriter::wrap_output(
                &mut encrypted,
                age::armor::Format::AsciiArmor,
            )?;

            let mut writer = encryptor
                .wrap_output(armored_writer)
                .context("failed to create age encryptor")?;

            writer
                .write_all(plaintext)
                .context("failed to write plaintext to age encryptor")?;

            writer
                .finish()
                .and_then(|w| w.finish())
                .context("failed to finish age encryption")?;
        }

        Ok(encrypted)
    }

    fn decrypt(&self, ciphertext: &[u8]) -> Result<Vec<u8>> {
        let identities = self.read_identities()?;
        let identity_refs: Vec<&dyn age::Identity> = identities
            .iter()
            .map(|i| i as &dyn age::Identity)
            .collect();

        // Try to parse as armored first
        let reader = std::io::Cursor::new(ciphertext);
        let armored = age::armor::ArmoredReader::new(reader);

        let decryptor = age::Decryptor::new(armored)
            .map_err(|e| anyhow!("failed to parse age ciphertext: {}", e))?;

        // Check if this is a passphrase-encrypted file
        if decryptor.is_scrypt() {
            bail!("passphrase-encrypted files are not supported");
        }

        let mut decrypted = Vec::new();
        let mut dec_reader = decryptor
            .decrypt(identity_refs.into_iter())
            .context("failed to decrypt with age identity")?;

        dec_reader
            .read_to_end(&mut decrypted)
            .context("failed to read decrypted data")?;

        Ok(decrypted)
    }

    fn derive_db_key(&self) -> Result<String> {
        // Derive DB key from the first identity's public key
        let identities = self.read_identities()?;
        let first_identity = identities.first().context("no identities available")?;

        // Get the public key from the identity
        let public_key = first_identity.to_public();

        // Hash the public key to create a deterministic DB key
        use sha1::{Digest, Sha1};
        let mut hasher = Sha1::new();
        hasher.update(b"rldx-sqlcipher-key-v1:");
        hasher.update(public_key.to_string().as_bytes());
        let hash = hasher.finalize();

        // Convert to hex string for SQLCipher PRAGMA key
        Ok(format!("x'{}'", hex::encode(hash)))
    }

    fn file_extension(&self) -> &'static str {
        "vcf.age"
    }

    fn encryption_type(&self) -> EncryptionType {
        EncryptionType::Age
    }
}

// =============================================================================
// Factory function
// =============================================================================

/// Create a CryptoProvider from configuration
#[allow(dead_code)] // Used once full encryption integration is complete
pub fn create_provider(config: &EncryptionConfig) -> Result<Box<dyn CryptoProvider>> {
    match config.encryption_type {
        EncryptionType::Gpg => {
            let key_id = config
                .gpg_key_id
                .clone()
                .context("gpg_key_id is required for GPG encryption")?;
            Ok(Box::new(GpgProvider::new(key_id)?))
        }
        EncryptionType::Age => {
            let identity_path = config
                .age_identity
                .clone()
                .context("age_identity is required for age encryption")?;
            let recipient = config
                .age_recipient
                .clone()
                .context("age_recipient is required for age encryption")?;
            Ok(Box::new(AgeProvider::new(identity_path, recipient)?))
        }
    }
}

// =============================================================================
// Helper functions for hex encoding
// =============================================================================

mod hex {
    const HEX_CHARS: &[u8; 16] = b"0123456789abcdef";

    pub fn encode(bytes: impl AsRef<[u8]>) -> String {
        let bytes = bytes.as_ref();
        let mut hex = String::with_capacity(bytes.len() * 2);
        for &b in bytes {
            hex.push(HEX_CHARS[(b >> 4) as usize] as char);
            hex.push(HEX_CHARS[(b & 0x0f) as usize] as char);
        }
        hex
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hex_encode() {
        assert_eq!(hex::encode([0x00]), "00");
        assert_eq!(hex::encode([0xff]), "ff");
        assert_eq!(hex::encode([0xde, 0xad, 0xbe, 0xef]), "deadbeef");
    }
}
