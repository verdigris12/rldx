use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{anyhow, Context, Result};
use sha1::{Digest, Sha1};
use uuid::Uuid;

use crate::vcard_io::{self, CardWithSource};

const NORMALIZED_MARKER: &str = ".rldx_normalized";

#[derive(Debug, Clone)]
pub struct NormalizedCard {
    pub uuid: Uuid,
    pub path: PathBuf,
}

#[derive(Debug, Default, Clone)]
pub struct NormalizationReport {
    pub cards: Vec<NormalizedCard>,
    pub needs_upgrade: Vec<PathBuf>,
    pub marker_created: bool,
}

pub fn marker_path(vdir: &Path) -> PathBuf {
    vdir.join(NORMALIZED_MARKER)
}

pub fn is_normalized(vdir: &Path) -> bool {
    marker_path(vdir).exists()
}

pub fn normalize(vdir: &Path) -> Result<NormalizationReport> {
    let mut report = NormalizationReport::default();

    if !vdir.exists() {
        return Err(anyhow!("vdir does not exist: {}", vdir.display()));
    }

    let marker = marker_path(vdir);
    if marker.exists() {
        return Ok(report);
    }

    let mut used_names = existing_stems(vdir)?;
    let mut files_to_remove: Vec<PathBuf> = Vec::new();

    let mut entries = list_vcf_files(vdir)?;
    entries.sort();

    for path in entries {
        let content = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        match vcard_io::parse_str_with_source(&content) {
            Ok(cards) => {
                process_cards(
                    vdir,
                    &path,
                    cards,
                    &mut used_names,
                    &mut report,
                    &mut files_to_remove,
                )?;
            }
            Err(err) => {
                eprintln!(
                    "warning: unable to parse vCard file {}: {err}",
                    path.display()
                );
            }
        }
    }

    for path in files_to_remove {
        if path.exists() {
            if let Err(err) = fs::remove_file(&path) {
                eprintln!(
                    "warning: failed to remove original file {}: {err}",
                    path.display()
                );
            }
        }
    }

    if !marker.exists() {
        if let Err(err) = fs::write(&marker, b"") {
            eprintln!(
                "warning: failed to create normalization marker {}: {err}",
                marker.display()
            );
        } else {
            report.marker_created = true;
        }
    }

    Ok(report)
}

fn process_cards(
    vdir: &Path,
    original_path: &Path,
    cards: Vec<CardWithSource>,
    used_names: &mut HashSet<String>,
    report: &mut NormalizationReport,
    files_to_remove: &mut Vec<PathBuf>,
) -> Result<()> {
    let multi = cards.len() > 1;
    let mut can_remove_original = true;
    let mut wrote_any = false;
    let mut wrote_to_different_path = false;
    let original_stem = original_path
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string());

    for card_src in cards {
        if !card_src.is_v4 {
            if !report.needs_upgrade.iter().any(|p| p == original_path) {
                report.needs_upgrade.push(original_path.to_path_buf());
            }
            can_remove_original = false;
            continue;
        }

        let mut card = card_src.card.clone();
        let uuid = vcard_io::ensure_uuid_uid(&mut card)?;
        vcard_io::touch_rev(&mut card);
        let short_name = select_filename(&uuid, used_names, original_stem.as_deref());
        let target = vdir.join(format!("{short_name}.vcf"));

        let bytes = vcard_io::card_to_bytes(&card);
        write_atomic(&target, &bytes)?;
        wrote_any = true;

        report.cards.push(NormalizedCard {
            uuid,
            path: target.clone(),
        });

        if target != *original_path {
            wrote_to_different_path = true;
        }
    }

    if can_remove_original
        && wrote_any
        && original_path.exists()
        && (multi || wrote_to_different_path)
    {
        files_to_remove.push(original_path.to_path_buf());
    }

    Ok(())
}

pub(crate) fn select_filename(
    uuid: &Uuid,
    used_names: &mut HashSet<String>,
    original_stem: Option<&str>,
) -> String {
    let hex = uuid.to_string().replace('-', "");
    let candidate_lengths = [12_usize, 16, 20, 24, 28, 32];

    for &len in &candidate_lengths {
        let candidate = &hex[..len.min(hex.len())];
        if Some(candidate) == original_stem {
            used_names.insert(candidate.to_string());
            return candidate.to_string();
        }
        if !used_names.contains(candidate) {
            used_names.insert(candidate.to_string());
            return candidate.to_string();
        }
    }

    if Some(hex.as_str()) == original_stem || !used_names.contains(&hex) {
        used_names.insert(hex.clone());
        return hex;
    }

    let mut counter = 1u32;
    loop {
        let candidate = format!("{}-{:x}", hex, counter);
        if !used_names.contains(&candidate) {
            used_names.insert(candidate.clone());
            return candidate;
        }
        counter += 1;
    }
}

pub(crate) fn existing_stems(vdir: &Path) -> Result<HashSet<String>> {
    let mut stems = HashSet::new();
    let mut files = list_vcf_files(vdir)?;
    files.sort();
    for path in files {
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            stems.insert(stem.to_string());
        }
    }
    Ok(stems)
}

pub fn list_vcf_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_vcf(root, &mut files)?;
    Ok(files)
}

fn collect_vcf(dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in
        fs::read_dir(dir).with_context(|| format!("failed to read directory {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_vcf(&path, files)?;
        } else if path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("vcf"))
            .unwrap_or(false)
        {
            files.push(path);
        }
    }
    Ok(())
}

pub struct FileState {
    pub sha1: Vec<u8>,
    pub mtime: i64,
}

pub fn compute_file_state(path: &Path) -> Result<FileState> {
    let metadata = fs::metadata(path)
        .with_context(|| format!("failed to read metadata for {}", path.display()))?;
    let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    let mtime = modified
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    let data = fs::read(path).with_context(|| format!("failed to read file {}", path.display()))?;
    let mut hasher = Sha1::new();
    hasher.update(&data);
    let sha1 = hasher.finalize().to_vec();

    Ok(FileState { sha1, mtime })
}

pub fn write_atomic(target: &Path, data: &[u8]) -> Result<()> {
    let parent = target
        .parent()
        .ok_or_else(|| anyhow!("target path has no parent: {}", target.display()))?;
    fs::create_dir_all(parent)
        .with_context(|| format!("failed to create parent dir {}", parent.display()))?;

    let mut temp_path = PathBuf::new();
    let mut counter: u32 = 0;
    loop {
        let candidate = if counter == 0 {
            target
                .file_name()
                .and_then(|name| name.to_str())
                .map(|name| format!(".{name}.tmp"))
                .unwrap_or_else(|| ".rldx.tmp".to_string())
        } else {
            target
                .file_name()
                .and_then(|name| name.to_str())
                .map(|name| format!(".{name}.{counter}.tmp"))
                .unwrap_or_else(|| format!(".rldx.{counter}.tmp"))
        };
        temp_path = parent.join(candidate);
        if !temp_path.exists() {
            break;
        }
        counter += 1;
    }

    {
        use std::fs::OpenOptions;
        use std::io::Write;

        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .with_context(|| {
                format!(
                    "failed to create temporary file {} for atomic write",
                    temp_path.display()
                )
            })?;

        file.write_all(data)
            .with_context(|| format!("failed to write temporary file {}", temp_path.display()))?;
        file.sync_all()
            .with_context(|| format!("failed to sync temporary file {}", temp_path.display()))?;
    }

    fs::rename(&temp_path, target).with_context(|| {
        format!(
            "failed to rename temporary file {} to {}",
            temp_path.display(),
            target.display()
        )
    })?;

    if let Ok(dir_file) = fs::File::open(parent) {
        let _ = dir_file.sync_all();
    }

    Ok(())
}
