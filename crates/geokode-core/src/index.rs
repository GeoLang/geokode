//! Text search index using Finite State Transducers (FST).
//!
//! Builds an FST from address strings for fast prefix/fuzzy lookup.

use fst::{Automaton, IntoStreamer, Map, MapBuilder, Streamer};
use std::io;

/// An FST-based text index mapping normalized address strings to record IDs.
pub struct TextIndex {
    map: Map<Vec<u8>>,
}

/// Builder for constructing a TextIndex.
pub struct TextIndexBuilder {
    builder: MapBuilder<Vec<u8>>,
    entries: Vec<(String, u64)>,
}

impl TextIndexBuilder {
    /// Create a new builder.
    pub fn new() -> Self {
        Self {
            builder: MapBuilder::memory(),
            entries: Vec::new(),
        }
    }

    /// Insert a key-value pair. Keys must be inserted in sorted order
    /// or call `build_sorted()` which sorts automatically.
    pub fn insert(&mut self, key: String, value: u64) {
        self.entries.push((key, value));
    }

    /// Build the index, sorting entries first.
    pub fn build(mut self) -> Result<TextIndex, io::Error> {
        self.entries.sort_by(|a, b| a.0.cmp(&b.0));
        self.entries.dedup_by(|a, b| a.0 == b.0);
        for (key, value) in &self.entries {
            self.builder
                .insert(key.as_bytes(), *value)
                .map_err(|e| io::Error::other(e.to_string()))?;
        }
        let bytes = self
            .builder
            .into_inner()
            .map_err(|e| io::Error::other(e.to_string()))?;
        let map = Map::new(bytes).map_err(|e| io::Error::other(e.to_string()))?;
        Ok(TextIndex { map })
    }
}

impl Default for TextIndexBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl TextIndex {
    /// Exact lookup.
    pub fn get(&self, key: &str) -> Option<u64> {
        self.map.get(key.as_bytes())
    }

    /// Prefix search — returns all entries starting with the given prefix.
    pub fn prefix_search(&self, prefix: &str) -> Vec<(String, u64)> {
        let automaton = fst::automaton::Str::new(prefix).starts_with();
        let mut stream = self.map.search(automaton).into_stream();
        let mut results = Vec::new();
        while let Some((key, value)) = stream.next() {
            if let Ok(s) = std::str::from_utf8(key) {
                results.push((s.to_string(), value));
            }
        }
        results
    }

    /// Number of entries in the index.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Whether the index is empty.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_and_lookup() {
        let mut builder = TextIndexBuilder::new();
        builder.insert("123 main st".to_string(), 0);
        builder.insert("456 oak ave".to_string(), 1);
        builder.insert("789 elm dr".to_string(), 2);
        let index = builder.build().unwrap();

        assert_eq!(index.get("123 main st"), Some(0));
        assert_eq!(index.get("456 oak ave"), Some(1));
        assert_eq!(index.get("nonexistent"), None);
        assert_eq!(index.len(), 3);
    }

    #[test]
    fn prefix_search_works() {
        let mut builder = TextIndexBuilder::new();
        builder.insert("main st".to_string(), 0);
        builder.insert("main ave".to_string(), 1);
        builder.insert("maple dr".to_string(), 2);
        builder.insert("oak ln".to_string(), 3);
        let index = builder.build().unwrap();

        let results = index.prefix_search("main");
        assert_eq!(results.len(), 2);
    }
}
