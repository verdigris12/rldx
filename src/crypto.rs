//! Encryption providers for vCard and database encryption.
//!
//! Supports two backends:
//! - GPG: Derives a symmetric key from the GPG key fingerprint using HKDF-SHA256,
//!        then uses orion's AEAD (XChaCha20-Poly1305) for fast file encryption.
//!        Files are stored as .vcf.age (text format with magic header).
//! - Age: Modern encryption using X25519 keys, stores files as .vcf.age

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use hkdf::Hkdf;
use sha2::Sha256;

use crate::config::{EncryptionConfig, EncryptionType};

// =============================================================================
// CryptoProvider Trait
// =============================================================================

/// Trait for encryption/decryption providers
pub trait CryptoProvider: Send + Sync {
    /// Encrypt plaintext data, returning ciphertext
    fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>>;

    /// Decrypt ciphertext, returning plaintext
    fn decrypt(&self, ciphertext: &[u8]) -> Result<Vec<u8>>;

    /// Derive a deterministic key for SQLCipher database encryption
    /// The key should be derived from the encryption key material
    fn derive_db_key(&self) -> Result<String>;

    /// Get the encryption type
    fn encryption_type(&self) -> EncryptionType;
}

// =============================================================================
// GPG Provider
// =============================================================================

/// Salt for HKDF key derivation (file encryption)
const HKDF_SALT_FILE: &[u8] = b"rldx-gpg-file-encryption-v2";
/// Salt for HKDF key derivation (database encryption)
const HKDF_SALT_DB: &[u8] = b"rldx-gpg-db-encryption-v1";
/// Magic header for GPG-derived encrypted files (using orion's AEAD)
const GPG_CHACHA_MAGIC: &[u8] = b"RLDX-GPG-CHACHA20\n";

/// GPG hybrid encryption provider.
/// 
/// Uses the GPG key fingerprint to derive a symmetric key via HKDF-SHA256,
/// then uses orion's AEAD (XChaCha20-Poly1305) for actual file encryption.
/// This provides:
/// - Fast encryption (no subprocess spawning, pure Rust crypto)
/// - Parallelizable operations  
/// - Security tied to GPG key possession (fingerprint is only accessible with the key)
/// - AEAD ensures authenticity and integrity
/// - High-level API handles nonces automatically
pub struct GpgProvider {
    /// The derived secret key for orion AEAD (cached at construction)
    secret_key: orion::aead::SecretKey,
    /// The GPG key fingerprint (cached for DB key derivation)
    fingerprint: String,
}

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

        // Get and cache the fingerprint
        let fingerprint = Self::fetch_key_fingerprint(&key_id)?;
        
        // Derive the encryption key using HKDF-SHA256
        let secret_key = Self::derive_secret_key(&fingerprint)?;

        Ok(Self {
            secret_key,
            fingerprint,
        })
    }

    /// Fetch the key fingerprint from GPG
    fn fetch_key_fingerprint(key_id: &str) -> Result<String> {
        let output = Command::new("gpg")
            .args([
                "--with-colons",
                "--fingerprint",
                key_id,
            ])
            .output()
            .context("failed to get GPG key fingerprint")?;

        if !output.status.success() {
            bail!("failed to get fingerprint for key '{}'", key_id);
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

        bail!("could not parse fingerprint for key '{}'", key_id)
    }

    /// Derive an orion SecretKey from the fingerprint using HKDF-SHA256
    fn derive_secret_key(fingerprint: &str) -> Result<orion::aead::SecretKey> {
        let hk = Hkdf::<Sha256>::new(Some(HKDF_SALT_FILE), fingerprint.as_bytes());
        
        let mut key_bytes = [0u8; 32];
        hk.expand(b"xchacha20-poly1305-key", &mut key_bytes)
            .map_err(|_| anyhow!("HKDF expansion failed"))?;
        
        orion::aead::SecretKey::from_slice(&key_bytes)
            .map_err(|_| anyhow!("failed to create orion SecretKey"))
    }
}

impl CryptoProvider for GpgProvider {
    fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>> {
        // Use orion's seal() - handles nonce generation automatically
        let ciphertext = orion::aead::seal(&self.secret_key, plaintext)
            .map_err(|_| anyhow!("encryption failed"))?;
        
        // Format: MAGIC || base64(ciphertext)
        let mut result = Vec::from(GPG_CHACHA_MAGIC);
        result.extend_from_slice(base64_encode(&ciphertext).as_bytes());
        result.push(b'\n');
        
        Ok(result)
    }

    fn decrypt(&self, ciphertext: &[u8]) -> Result<Vec<u8>> {
        // Check for our magic header
        if !ciphertext.starts_with(GPG_CHACHA_MAGIC) {
            bail!("invalid file format: missing GPG-ChaCha20 header");
        }
        
        // Strip magic header and decode base64
        let encoded = &ciphertext[GPG_CHACHA_MAGIC.len()..];
        let encoded_str = std::str::from_utf8(encoded)
            .context("invalid UTF-8 in ciphertext")?
            .trim();
        let decoded = base64_decode(encoded_str)
            .context("failed to decode base64 ciphertext")?;
        
        // Use orion's open() - handles nonce extraction automatically
        orion::aead::open(&self.secret_key, &decoded)
            .map_err(|_| anyhow!("decryption failed - invalid key or corrupted data"))
    }

    fn derive_db_key(&self) -> Result<String> {
        // Use HKDF-SHA256 for DB key derivation (separate from file encryption key)
        let hk = Hkdf::<Sha256>::new(Some(HKDF_SALT_DB), self.fingerprint.as_bytes());
        
        // Derive 32 bytes for SQLCipher key
        let mut okm = [0u8; 32];
        hk.expand(b"sqlcipher-key", &mut okm)
            .map_err(|_| anyhow!("HKDF expansion failed for DB key"))?;

        // Convert to hex string for SQLCipher PRAGMA key
        Ok(format!("x'{}'", hex_encode(&okm)))
    }

    fn encryption_type(&self) -> EncryptionType {
        EncryptionType::Gpg
    }
}

// =============================================================================
// Age Provider
// =============================================================================

/// Age encryption provider using the age library
pub struct AgeProvider {
    identity_path: PathBuf,
    recipient: String,
}

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

    /// Create a new ephemeral AgeProvider for testing.
    /// Generates a new identity and writes it to a temp file.
    #[cfg(test)]
    pub fn new_ephemeral(temp_dir: &std::path::Path) -> Result<Self> {
        use age::secrecy::ExposeSecret;
        
        let identity = age::x25519::Identity::generate();
        let recipient = identity.to_public().to_string();
        
        let identity_content = format!(
            "# test identity\n{}\n",
            identity.to_string().expose_secret()
        );
        
        let identity_path = temp_dir.join("age-identity.txt");
        std::fs::write(&identity_path, identity_content)?;
        
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
        // Derive DB key from the first identity's public key using HKDF-SHA256
        let identities = self.read_identities()?;
        let first_identity = identities.first().context("no identities available")?;

        // Get the public key from the identity
        let public_key = first_identity.to_public();

        // Use HKDF-SHA256 for key derivation
        let hk = Hkdf::<Sha256>::new(
            Some(b"rldx-age-db-encryption-v1"),
            public_key.to_string().as_bytes(),
        );
        
        // Derive 32 bytes for SQLCipher key
        let mut okm = [0u8; 32];
        hk.expand(b"sqlcipher-key", &mut okm)
            .map_err(|_| anyhow!("HKDF expansion failed for DB key"))?;

        // Convert to hex string for SQLCipher PRAGMA key
        Ok(format!("x'{}'", hex_encode(&okm)))
    }

    fn encryption_type(&self) -> EncryptionType {
        EncryptionType::Age
    }
}

// =============================================================================
// Factory function
// =============================================================================

/// Create a CryptoProvider from configuration
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
// Helper functions for encoding
// =============================================================================

fn hex_encode(bytes: &[u8]) -> String {
    const HEX_CHARS: &[u8; 16] = b"0123456789abcdef";
    let mut hex = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        hex.push(HEX_CHARS[(b >> 4) as usize] as char);
        hex.push(HEX_CHARS[(b & 0x0f) as usize] as char);
    }
    hex
}

fn base64_encode(bytes: &[u8]) -> String {
    use base64::{Engine, engine::general_purpose::STANDARD};
    STANDARD.encode(bytes)
}

fn base64_decode(s: &str) -> Result<Vec<u8>> {
    use base64::{Engine, engine::general_purpose::STANDARD};
    STANDARD.decode(s).map_err(|e| anyhow!("base64 decode error: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hex_encode() {
        assert_eq!(hex_encode(&[0x00]), "00");
        assert_eq!(hex_encode(&[0xff]), "ff");
        assert_eq!(hex_encode(&[0xde, 0xad, 0xbe, 0xef]), "deadbeef");
    }

    #[test]
    fn test_base64_encode() {
        assert_eq!(base64_encode(&[0x00, 0x01, 0x02]), "AAEC");
        assert_eq!(base64_encode(b"hello"), "aGVsbG8=");
    }

    #[test]
    fn test_base64_roundtrip() {
        let data = b"hello world!";
        let encoded = base64_encode(data);
        let decoded = base64_decode(&encoded).unwrap();
        assert_eq!(decoded, data);
    }

    /// Helper to create a test GpgProvider without requiring actual GPG
    fn test_provider(fingerprint: &str) -> GpgProvider {
        let secret_key = GpgProvider::derive_secret_key(fingerprint).unwrap();
        GpgProvider {
            secret_key,
            fingerprint: fingerprint.to_string(),
        }
    }

    #[test]
    fn test_gpg_encrypt_decrypt_roundtrip() {
        let provider = test_provider("TEST_FINGERPRINT_12345");

        let plaintext = b"Hello, this is a test vCard content!";
        
        // Encrypt
        let ciphertext = provider.encrypt(plaintext).unwrap();
        
        // Verify it starts with our magic header
        assert!(ciphertext.starts_with(GPG_CHACHA_MAGIC));
        
        // Decrypt
        let decrypted = provider.decrypt(&ciphertext).unwrap();
        
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_gpg_different_keys_fail() {
        let provider1 = test_provider("FINGERPRINT_ONE");
        let provider2 = test_provider("FINGERPRINT_TWO");

        let plaintext = b"Secret data";
        let ciphertext = provider1.encrypt(plaintext).unwrap();
        
        // Decrypting with wrong key should fail
        let result = provider2.decrypt(&ciphertext);
        assert!(result.is_err());
    }

    #[test]
    fn test_gpg_deterministic_key_derivation() {
        // Same fingerprint should always produce the same key
        let provider1 = test_provider("SAME_FINGERPRINT");
        let provider2 = test_provider("SAME_FINGERPRINT");
        
        let plaintext = b"Test message";
        let ciphertext = provider1.encrypt(plaintext).unwrap();
        
        // Should be able to decrypt with provider2 (same derived key)
        let decrypted = provider2.decrypt(&ciphertext).unwrap();
        assert_eq!(decrypted, plaintext);
    }
}
