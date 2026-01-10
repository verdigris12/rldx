//! Integration tests for rldx init and import commands

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use assert_cmd::Command as AssertCommand;
use predicates::prelude::*;
use tempfile::TempDir;

// =============================================================================
// Test Helpers
// =============================================================================

/// Test environment with initialized rldx config and vdir
struct TestEnv {
    temp_dir: TempDir,
    config_path: PathBuf,
    vdir_path: PathBuf,
    gnupg_home: Option<PathBuf>,
}

impl TestEnv {
    /// Create a new test environment with age encryption
    fn new_with_age() -> Self {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.toml");
        let vdir_path = temp_dir.path().join("vdir");
        let db_path = temp_dir.path().join("index.db");

        // Run init
        rldx_cmd()
            .args([
                "init",
                "--config",
                config_path.to_str().unwrap(),
                "--encryption",
                "age",
                vdir_path.to_str().unwrap(),
            ])
            .assert()
            .success();

        // Update config to use isolated db_path
        let config_content = fs::read_to_string(&config_path).unwrap();
        let updated_config = update_db_path_in_config(&config_content, &db_path);
        fs::write(&config_path, updated_config).unwrap();

        Self {
            temp_dir,
            config_path,
            vdir_path,
            gnupg_home: None,
        }
    }

    /// Create a new test environment with GPG encryption
    fn new_with_gpg() -> Self {
        let temp_dir = TempDir::new().unwrap();
        let gnupg_home = temp_dir.path().join("gnupg");
        let key_id = generate_test_gpg_key(&gnupg_home);

        let config_path = temp_dir.path().join("config.toml");
        let vdir_path = temp_dir.path().join("vdir");
        let db_path = temp_dir.path().join("index.db");

        // Run init with GNUPGHOME set
        rldx_cmd()
            .env("GNUPGHOME", &gnupg_home)
            .args([
                "init",
                "--config",
                config_path.to_str().unwrap(),
                "--encryption",
                "gpg",
                "--key",
                &key_id,
                vdir_path.to_str().unwrap(),
            ])
            .assert()
            .success();

        // Update config to use isolated db_path
        let config_content = fs::read_to_string(&config_path).unwrap();
        let updated_config = update_db_path_in_config(&config_content, &db_path);
        fs::write(&config_path, updated_config).unwrap();

        Self {
            temp_dir,
            config_path,
            vdir_path,
            gnupg_home: Some(gnupg_home),
        }
    }

    /// Run rldx with this test env's config
    fn rldx(&self) -> AssertCommand {
        let mut cmd = rldx_cmd();
        cmd.args(["--config", self.config_path.to_str().unwrap()]);
        // If GPG, set GNUPGHOME
        if let Some(ref gnupg_home) = self.gnupg_home {
            cmd.env("GNUPGHOME", gnupg_home);
        }
        cmd
    }
}

/// Get the rldx binary command
fn rldx_cmd() -> AssertCommand {
    AssertCommand::cargo_bin("rldx").unwrap()
}

/// Check if gpg is available
fn gpg_available() -> bool {
    Command::new("gpg")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Generate a test GPG key and return the key ID
fn generate_test_gpg_key(gnupg_home: &Path) -> String {
    fs::create_dir_all(gnupg_home).unwrap();

    // Set restrictive permissions on gnupg home (GPG requires this)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o700);
        fs::set_permissions(gnupg_home, perms).unwrap();
    }

    // Batch key generation (no passphrase, no interaction)
    let batch_config = r#"%no-protection
Key-Type: RSA
Key-Length: 2048
Name-Real: Test User
Name-Email: test@example.com
Expire-Date: 0
%commit
"#;

    let batch_file = gnupg_home.join("keygen.batch");
    fs::write(&batch_file, batch_config).unwrap();

    let output = Command::new("gpg")
        .env("GNUPGHOME", gnupg_home)
        .args(["--batch", "--gen-key", batch_file.to_str().unwrap()])
        .output()
        .expect("failed to generate GPG key");

    if !output.status.success() {
        panic!(
            "GPG key generation failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // Get the key ID
    let output = Command::new("gpg")
        .env("GNUPGHOME", gnupg_home)
        .args(["--list-keys", "--with-colons"])
        .output()
        .unwrap();

    // Parse key ID from output (pub:...:...:...:KEYID:...)
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        if line.starts_with("pub:") {
            let parts: Vec<&str> = line.split(':').collect();
            if parts.len() > 4 {
                return parts[4].to_string();
            }
        }
    }
    panic!(
        "failed to get GPG key ID from output: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

/// Path to test data directory
fn test_data_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests")
}

/// Path to test maildir
fn test_maildir_path() -> PathBuf {
    test_data_dir().join("test_maildir")
}

/// Path to test contacts VCF file
fn test_contacts_vcf_path() -> PathBuf {
    test_data_dir().join("test_contacts.vcf")
}

/// Update db_path in config content to use an isolated test database
fn update_db_path_in_config(config_content: &str, db_path: &Path) -> String {
    let mut lines: Vec<String> = config_content.lines().map(|l| l.to_string()).collect();
    
    for line in &mut lines {
        if line.starts_with("db_path") {
            *line = format!("db_path = \"{}\"", db_path.display());
            break;
        }
    }
    
    lines.join("\n")
}

// =============================================================================
// Init Tests
// =============================================================================

#[test]
fn test_init_age_creates_config_and_vdir() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.toml");
    let vdir_path = temp_dir.path().join("vdir");

    rldx_cmd()
        .args([
            "init",
            "--config",
            config_path.to_str().unwrap(),
            "--encryption",
            "age",
            vdir_path.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Initialized rldx with age encryption"));

    // Config exists and contains encryption section
    assert!(config_path.exists());
    let config_content = fs::read_to_string(&config_path).unwrap();
    assert!(config_content.contains(r#"type = "age""#));
    assert!(config_content.contains("age_identity"));
    assert!(config_content.contains("age_recipient"));

    // Identity file exists in same directory as config
    let identity_path = temp_dir.path().join("age-identity.txt");
    assert!(identity_path.exists());

    // vdir exists and is empty
    assert!(vdir_path.exists());
    assert!(fs::read_dir(&vdir_path).unwrap().count() == 0);
}

#[test]
fn test_init_gpg_creates_config() {
    if !gpg_available() {
        eprintln!("Skipping test: gpg not available");
        return;
    }
    
    let temp_dir = TempDir::new().unwrap();
    let gnupg_home = temp_dir.path().join("gnupg");
    let key_id = generate_test_gpg_key(&gnupg_home);

    let config_path = temp_dir.path().join("config.toml");
    let vdir_path = temp_dir.path().join("vdir");

    rldx_cmd()
        .env("GNUPGHOME", &gnupg_home)
        .args([
            "init",
            "--config",
            config_path.to_str().unwrap(),
            "--encryption",
            "gpg",
            "--key",
            &key_id,
            vdir_path.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Initialized rldx with gpg encryption"));

    // Config contains GPG settings
    let config_content = fs::read_to_string(&config_path).unwrap();
    assert!(config_content.contains(r#"type = "gpg""#));
    assert!(config_content.contains(&key_id));
}

#[test]
fn test_init_fails_if_config_exists_without_force() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.toml");
    let vdir_path = temp_dir.path().join("vdir");

    // Create existing config
    fs::write(&config_path, "existing config").unwrap();

    rldx_cmd()
        .args([
            "init",
            "--config",
            config_path.to_str().unwrap(),
            "--encryption",
            "age",
            vdir_path.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Configuration already exists"));
}

#[test]
fn test_init_force_overwrites_existing_config() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.toml");
    let vdir_path = temp_dir.path().join("vdir");

    // Create existing config
    fs::write(&config_path, "old config").unwrap();

    rldx_cmd()
        .args([
            "init",
            "--config",
            config_path.to_str().unwrap(),
            "--encryption",
            "age",
            "--force",
            vdir_path.to_str().unwrap(),
        ])
        .assert()
        .success();

    // Config was overwritten
    let config_content = fs::read_to_string(&config_path).unwrap();
    assert!(config_content.contains(r#"type = "age""#));
}

#[test]
fn test_init_fails_if_vdir_not_empty() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.toml");
    let vdir_path = temp_dir.path().join("vdir");

    // Create non-empty vdir
    fs::create_dir_all(&vdir_path).unwrap();
    fs::write(vdir_path.join("existing.txt"), "data").unwrap();

    rldx_cmd()
        .args([
            "init",
            "--config",
            config_path.to_str().unwrap(),
            "--encryption",
            "age",
            vdir_path.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not empty"));
}

#[test]
fn test_init_gpg_fails_without_key() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.toml");
    let vdir_path = temp_dir.path().join("vdir");

    rldx_cmd()
        .args([
            "init",
            "--config",
            config_path.to_str().unwrap(),
            "--encryption",
            "gpg",
            vdir_path.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--key is required"));
}

#[test]
fn test_init_creates_db_path_in_config() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.toml");
    let vdir_path = temp_dir.path().join("vdir");

    rldx_cmd()
        .args([
            "init",
            "--config",
            config_path.to_str().unwrap(),
            "--encryption",
            "age",
            vdir_path.to_str().unwrap(),
        ])
        .assert()
        .success();

    // Config contains db_path
    let config_content = fs::read_to_string(&config_path).unwrap();
    assert!(config_content.contains("db_path"));
}

// =============================================================================
// Import Tests
// =============================================================================

#[test]
fn test_import_google_contacts() {
    let env = TestEnv::new_with_age();

    env.rldx()
        .args([
            "import",
            "--format",
            "google",
            test_contacts_vcf_path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Imported 50 contacts"));

    // Check vdir has encrypted files (filter out non-vcf files like .rldx_normalized)
    let vdir_entries: Vec<_> = fs::read_dir(&env.vdir_path)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.ends_with(".vcf.age") || name.ends_with(".vcf.gpg")
        })
        .collect();
    assert_eq!(vdir_entries.len(), 50);

    // All vcf files should be .vcf.age
    for entry in vdir_entries {
        let name = entry.file_name().to_string_lossy().to_string();
        assert!(
            name.ends_with(".vcf.age"),
            "Expected .vcf.age file, got {}",
            name
        );
    }
}

#[test]
fn test_import_google_contacts_with_gpg() {
    if !gpg_available() {
        eprintln!("Skipping test: gpg not available");
        return;
    }
    
    let env = TestEnv::new_with_gpg();

    env.rldx()
        .args([
            "import",
            "--format",
            "google",
            test_contacts_vcf_path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Imported 50 contacts"));

    // All vcf files should be .vcf.age (GPG mode now uses Age format internally)
    let vcf_files: Vec<_> = fs::read_dir(&env.vdir_path)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.ends_with(".vcf.age") || name.ends_with(".vcf.gpg")
        })
        .collect();

    assert_eq!(vcf_files.len(), 50);

    for entry in vcf_files {
        let name = entry.file_name().to_string_lossy().to_string();
        assert!(
            name.ends_with(".vcf.age"),
            "Expected .vcf.age file (GPG now uses Age format), got {}",
            name
        );
    }
}

#[test]
fn test_import_maildir() {
    let env = TestEnv::new_with_age();

    env.rldx()
        .args([
            "import",
            "--format",
            "maildir",
            test_maildir_path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Imported")); // Count may vary due to deduplication

    // Check some contacts were created
    let vdir_entries: Vec<_> = fs::read_dir(&env.vdir_path)
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    assert!(!vdir_entries.is_empty(), "Expected contacts to be imported");
}

#[test]
fn test_import_to_address_book() {
    let env = TestEnv::new_with_age();

    env.rldx()
        .args([
            "import",
            "--format",
            "google",
            "--book",
            "work",
            test_contacts_vcf_path().to_str().unwrap(),
        ])
        .assert()
        .success();

    // Check contacts are in work subdirectory
    let work_dir = env.vdir_path.join("work");
    assert!(work_dir.exists(), "work subdirectory should exist");

    let entries: Vec<_> = fs::read_dir(&work_dir)
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    assert_eq!(entries.len(), 50);
}

#[test]
fn test_query_after_import() {
    let env = TestEnv::new_with_age();

    // First import contacts
    env.rldx()
        .args([
            "import",
            "--format",
            "google",
            test_contacts_vcf_path().to_str().unwrap(),
        ])
        .assert()
        .success();

    // Query for a specific contact
    env.rldx()
        .args(["query", "zane"])
        .assert()
        .success()
        .stdout(predicate::str::contains("zane.miller@blueskycorp.com"));
}

#[test]
fn test_import_google_no_errors() {
    let env = TestEnv::new_with_age();

    env.rldx()
        .args([
            "import",
            "--format",
            "google",
            test_contacts_vcf_path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stderr(predicate::str::is_empty()); // No errors
}

#[test]
fn test_import_maildir_no_errors() {
    let env = TestEnv::new_with_age();

    // Maildir import outputs progress to stderr, which is not an error
    // Just check that it succeeds and doesn't contain "Error"
    env.rldx()
        .args([
            "import",
            "--format",
            "maildir",
            test_maildir_path().to_str().unwrap(),
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("Error").not());
}
