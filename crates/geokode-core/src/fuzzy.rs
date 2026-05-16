//! Fuzzy and phonetic matching for geocoding queries.
//!
//! Provides Levenshtein distance matching and Soundex phonetic encoding
//! to handle typos, alternate spellings, and phonetically similar addresses.

use serde::{Deserialize, Serialize};

/// Fuzzy match result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FuzzyMatch {
    pub text: String,
    pub score: f64,
    pub distance: usize,
    pub record_id: u64,
}

/// Fuzzy matching configuration.
#[derive(Debug, Clone)]
pub struct FuzzyConfig {
    /// Maximum edit distance for matches.
    pub max_distance: usize,
    /// Whether to use phonetic matching as fallback.
    pub phonetic_fallback: bool,
    /// Minimum score threshold (0.0-1.0).
    pub min_score: f64,
}

impl Default for FuzzyConfig {
    fn default() -> Self {
        Self {
            max_distance: 2,
            phonetic_fallback: true,
            min_score: 0.5,
        }
    }
}

/// Fuzzy search engine operating over a dictionary of address strings.
pub struct FuzzySearcher {
    entries: Vec<(String, u64)>,
    config: FuzzyConfig,
}

impl FuzzySearcher {
    pub fn new(config: FuzzyConfig) -> Self {
        Self {
            entries: Vec::new(),
            config,
        }
    }

    /// Add an entry to the fuzzy search dictionary.
    pub fn add_entry(&mut self, text: String, record_id: u64) {
        self.entries.push((text, record_id));
    }

    /// Search for fuzzy matches to a query.
    pub fn search(&self, query: &str, limit: usize) -> Vec<FuzzyMatch> {
        let normalized = query.to_lowercase();
        let query_soundex = soundex(&normalized);

        let mut matches: Vec<FuzzyMatch> = self
            .entries
            .iter()
            .filter_map(|(text, id)| {
                let text_lower = text.to_lowercase();
                let dist = levenshtein(&normalized, &text_lower);

                if dist <= self.config.max_distance {
                    let max_len = normalized.len().max(text_lower.len());
                    let score = if max_len == 0 {
                        1.0
                    } else {
                        1.0 - (dist as f64 / max_len as f64)
                    };

                    if score >= self.config.min_score {
                        return Some(FuzzyMatch {
                            text: text.clone(),
                            score,
                            distance: dist,
                            record_id: *id,
                        });
                    }
                }

                // Phonetic fallback
                if self.config.phonetic_fallback {
                    let entry_soundex = soundex(&text_lower);
                    if query_soundex == entry_soundex {
                        return Some(FuzzyMatch {
                            text: text.clone(),
                            score: 0.6, // Phonetic match has lower confidence
                            distance: dist,
                            record_id: *id,
                        });
                    }
                }

                None
            })
            .collect();

        matches.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        matches.truncate(limit);
        matches
    }

    /// Number of entries in the dictionary.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Compute the Levenshtein edit distance between two strings.
pub fn levenshtein(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let m = a_chars.len();
    let n = b_chars.len();

    if m == 0 {
        return n;
    }
    if n == 0 {
        return m;
    }

    let mut prev: Vec<usize> = (0..=n).collect();
    let mut curr = vec![0usize; n + 1];

    for i in 1..=m {
        curr[0] = i;
        for j in 1..=n {
            let cost = if a_chars[i - 1] == b_chars[j - 1] {
                0
            } else {
                1
            };
            curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[n]
}

/// Compute the Soundex phonetic code for a string.
pub fn soundex(input: &str) -> String {
    let chars: Vec<char> = input.chars().filter(|c| c.is_ascii_alphabetic()).collect();
    if chars.is_empty() {
        return "0000".to_string();
    }

    let mut result = String::with_capacity(4);
    result.push(chars[0].to_ascii_uppercase());

    let code = |c: char| -> Option<char> {
        match c.to_ascii_lowercase() {
            'b' | 'f' | 'p' | 'v' => Some('1'),
            'c' | 'g' | 'j' | 'k' | 'q' | 's' | 'x' | 'z' => Some('2'),
            'd' | 't' => Some('3'),
            'l' => Some('4'),
            'm' | 'n' => Some('5'),
            'r' => Some('6'),
            _ => None,
        }
    };

    let mut last_code = code(chars[0]);

    for &c in &chars[1..] {
        if result.len() >= 4 {
            break;
        }
        let current_code = code(c);
        if let Some(cc) = current_code
            && Some(cc) != last_code
        {
            result.push(cc);
        }
        last_code = current_code;
    }

    while result.len() < 4 {
        result.push('0');
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_levenshtein_identical() {
        assert_eq!(levenshtein("hello", "hello"), 0);
    }

    #[test]
    fn test_levenshtein_single_edit() {
        assert_eq!(levenshtein("hello", "hallo"), 1);
        assert_eq!(levenshtein("cat", "cats"), 1);
    }

    #[test]
    fn test_levenshtein_multiple_edits() {
        assert_eq!(levenshtein("kitten", "sitting"), 3);
    }

    #[test]
    fn test_soundex_basic() {
        assert_eq!(soundex("Robert"), "R163");
        assert_eq!(soundex("Rupert"), "R163");
        assert_eq!(soundex("Smith"), "S530");
        assert_eq!(soundex("Smythe"), "S530");
    }

    #[test]
    fn test_soundex_similar_names() {
        // Soundex encodes these the same
        assert_eq!(soundex("Robert"), soundex("Rupert"));
        assert_eq!(soundex("Smith"), soundex("Smythe"));
    }

    #[test]
    fn test_fuzzy_search() {
        let mut searcher = FuzzySearcher::new(FuzzyConfig::default());
        searcher.add_entry("Main Street".into(), 1);
        searcher.add_entry("Main Avenue".into(), 2);
        searcher.add_entry("Oak Road".into(), 3);

        // Exact-ish match with typo
        let results = searcher.search("Main Streat", 5);
        assert!(!results.is_empty());
        assert_eq!(results[0].record_id, 1);
    }

    #[test]
    fn test_fuzzy_phonetic_fallback() {
        let mut searcher = FuzzySearcher::new(FuzzyConfig {
            max_distance: 1, // strict
            phonetic_fallback: true,
            min_score: 0.4,
        });
        searcher.add_entry("Smith Road".into(), 1);
        searcher.add_entry("Smythe Road".into(), 2);

        let results = searcher.search("Smyth Road", 5);
        assert!(!results.is_empty());
    }
}
