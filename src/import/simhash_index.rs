//! SimHash-based BK-tree index for fast fuzzy name matching during import.
//!
//! Uses SimHash to compute locality-sensitive hashes of normalized names,
//! then stores them in a BK-tree for O(log n) lookup by Hamming distance.
//!
//! Supports both FN (Formatted Name) and NICKNAME entries, with preference
//! given to FN matches during merge candidate selection.

use std::path::PathBuf;

use bktree::BkTree;

/// Hamming distance between two 64-bit SimHash values
fn hamming_distance(a: &u64, b: &u64) -> isize {
    (a ^ b).count_ones() as isize
}

/// Source of the name entry (FN or Nickname)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameSource {
    FN,
    Nickname,
}

impl NameSource {
    /// Parse from string (as stored in database)
    pub fn from_str(s: &str) -> Self {
        match s {
            "FN" => NameSource::FN,
            _ => NameSource::Nickname,
        }
    }
}

/// Entry stored in the BK-tree
#[derive(Debug, Clone)]
pub struct SimHashEntry {
    /// Path to the vCard file
    pub path: PathBuf,
    /// The contact's primary FN (for display when merging)
    pub display_fn: String,
    /// The normalized name that was indexed (could be FN or nickname)
    pub matched_norm: String,
    /// SimHash of the normalized name
    pub simhash: u64,
    /// Whether this entry is from FN or NICKNAME
    pub source: NameSource,
}

/// BK-tree index for SimHash-based fuzzy matching
pub struct SimHashIndex {
    tree: BkTree<SimHashEntry>,
}

impl SimHashIndex {
    /// Build a new index from database simhash entries
    /// Input: (path, display_fn, value_norm, simhash, source)
    pub fn new(entries: Vec<(PathBuf, String, String, u64, String)>) -> Self {
        let mut tree = BkTree::new(|a: &SimHashEntry, b: &SimHashEntry| {
            hamming_distance(&a.simhash, &b.simhash)
        });

        for (path, display_fn, matched_norm, simhash, source_str) in entries {
            tree.insert(SimHashEntry {
                path,
                display_fn,
                matched_norm,
                simhash,
                source: NameSource::from_str(&source_str),
            });
        }

        Self { tree }
    }

    /// Find all entries within the given Hamming distance threshold
    pub fn find_candidates(&self, simhash: u64, threshold: u32) -> Vec<&SimHashEntry> {
        let query = SimHashEntry {
            path: PathBuf::new(),
            display_fn: String::new(),
            matched_norm: String::new(),
            simhash,
            source: NameSource::FN,
        };

        self.tree
            .find(query, threshold as isize)
            .into_iter()
            .map(|(entry, _distance)| entry)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hamming_distance() {
        assert_eq!(hamming_distance(&0, &0), 0);
        assert_eq!(hamming_distance(&0, &1), 1);
        assert_eq!(hamming_distance(&0b1111, &0b0000), 4);
        assert_eq!(hamming_distance(&u64::MAX, &0), 64);
    }

    #[test]
    fn test_simhash_index_find_with_source() {
        let entries = vec![
            (
                PathBuf::from("/a.vcf"),
                "John Smith".into(),
                "john smith".into(),
                0b0000_0000u64,
                "FN".into(),
            ),
            (
                PathBuf::from("/a.vcf"),
                "John Smith".into(),
                "johnny".into(),
                0b0000_0011u64, // 2 bits diff
                "NICKNAME".into(),
            ),
            (
                PathBuf::from("/b.vcf"),
                "Jane Doe".into(),
                "jane doe".into(),
                0b1111_1111u64, // 8 bits diff
                "FN".into(),
            ),
        ];

        let index = SimHashIndex::new(entries);

        // Find within 2 bits - should get John Smith (FN) and johnny (NICKNAME)
        let candidates = index.find_candidates(0b0000_0000, 2);
        assert_eq!(candidates.len(), 2);

        // Verify we can distinguish FN from NICKNAME
        let fn_count = candidates
            .iter()
            .filter(|e| e.source == NameSource::FN)
            .count();
        let nick_count = candidates
            .iter()
            .filter(|e| e.source == NameSource::Nickname)
            .count();
        assert_eq!(fn_count, 1);
        assert_eq!(nick_count, 1);
    }

    #[test]
    fn test_name_source_from_str() {
        assert_eq!(NameSource::from_str("FN"), NameSource::FN);
        assert_eq!(NameSource::from_str("NICKNAME"), NameSource::Nickname);
        assert_eq!(NameSource::from_str("anything"), NameSource::Nickname);
    }
}
