//! Parallel batch geocoding with progress reporting.
//!
//! Processes large address lists concurrently using Rayon, with
//! callbacks for progress tracking and error handling.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde::{Deserialize, Serialize};

use crate::address::GeoResult;
use crate::geocode::Geocoder;

/// Result of a single batch geocoding operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchResult {
    pub index: usize,
    pub query: String,
    pub results: Vec<GeoResult>,
    pub status: BatchStatus,
}

/// Status of a batch item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BatchStatus {
    /// Successfully geocoded with at least one result.
    Matched,
    /// No results found.
    NoMatch,
    /// Error during processing.
    Error,
}

/// Summary statistics for a batch geocoding run.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BatchSummary {
    pub total: usize,
    pub matched: usize,
    pub no_match: usize,
    pub errors: usize,
    pub avg_confidence: f64,
}

/// Progress information for batch processing.
#[derive(Debug, Clone)]
pub struct BatchProgress {
    completed: Arc<AtomicUsize>,
    total: usize,
}

impl BatchProgress {
    fn new(total: usize) -> Self {
        Self {
            completed: Arc::new(AtomicUsize::new(0)),
            total,
        }
    }

    /// Get the number of completed items.
    pub fn completed(&self) -> usize {
        self.completed.load(Ordering::Relaxed)
    }

    /// Get the total number of items.
    pub fn total(&self) -> usize {
        self.total
    }

    /// Get progress as a fraction (0.0 - 1.0).
    pub fn fraction(&self) -> f64 {
        if self.total == 0 {
            return 1.0;
        }
        self.completed() as f64 / self.total as f64
    }

    fn increment(&self) {
        self.completed.fetch_add(1, Ordering::Relaxed);
    }
}

/// Batch geocoding processor.
pub struct BatchGeocoder<'a> {
    geocoder: &'a Geocoder,
    max_results_per_query: usize,
}

impl<'a> BatchGeocoder<'a> {
    pub fn new(geocoder: &'a Geocoder, max_results_per_query: usize) -> Self {
        Self {
            geocoder,
            max_results_per_query,
        }
    }

    /// Process a batch of address queries sequentially.
    pub fn process(&self, queries: &[&str]) -> (Vec<BatchResult>, BatchSummary) {
        let progress = BatchProgress::new(queries.len());
        let mut results = Vec::with_capacity(queries.len());

        for (index, query) in queries.iter().enumerate() {
            let geo_results = self.geocoder.forward(query);
            let limited: Vec<GeoResult> = geo_results
                .into_iter()
                .take(self.max_results_per_query)
                .collect();

            let status = if limited.is_empty() {
                BatchStatus::NoMatch
            } else {
                BatchStatus::Matched
            };

            results.push(BatchResult {
                index,
                query: query.to_string(),
                results: limited,
                status,
            });

            progress.increment();
        }

        let summary = compute_summary(&results);
        (results, summary)
    }

    /// Process a batch in parallel using Rayon.
    #[cfg(feature = "parallel")]
    pub fn process_parallel(&self, queries: &[&str]) -> (Vec<BatchResult>, BatchSummary) {
        use rayon::prelude::*;

        let progress = BatchProgress::new(queries.len());
        let results: Vec<BatchResult> = queries
            .par_iter()
            .enumerate()
            .map(|(index, query)| {
                let geo_results = self.geocoder.forward(query);
                let limited: Vec<GeoResult> = geo_results
                    .into_iter()
                    .take(self.max_results_per_query)
                    .collect();

                let status = if limited.is_empty() {
                    BatchStatus::NoMatch
                } else {
                    BatchStatus::Matched
                };

                progress.increment();

                BatchResult {
                    index,
                    query: query.to_string(),
                    results: limited,
                    status,
                }
            })
            .collect();

        let summary = compute_summary(&results);
        (results, summary)
    }

    /// Get the geocoder reference.
    pub fn geocoder(&self) -> &Geocoder {
        self.geocoder
    }
}

fn compute_summary(results: &[BatchResult]) -> BatchSummary {
    let total = results.len();
    let matched = results
        .iter()
        .filter(|r| r.status == BatchStatus::Matched)
        .count();
    let no_match = results
        .iter()
        .filter(|r| r.status == BatchStatus::NoMatch)
        .count();
    let errors = results
        .iter()
        .filter(|r| r.status == BatchStatus::Error)
        .count();

    let confidence_sum: f64 = results
        .iter()
        .filter_map(|r| r.results.first().map(|g| g.confidence))
        .sum();
    let avg_confidence = if matched > 0 {
        confidence_sum / matched as f64
    } else {
        0.0
    };

    BatchSummary {
        total,
        matched,
        no_match,
        errors,
        avg_confidence,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::address::Address;
    use crate::geocode::GeocoderBuilder;

    fn test_geocoder() -> Geocoder {
        let mut builder = GeocoderBuilder::new();
        builder.add(
            Address {
                house_number: Some("123".into()),
                street: Some("Main Street".into()),
                city: Some("Springfield".into()),
                state: Some("IL".into()),
                postcode: Some("62701".into()),
                country: Some("US".into()),
                full: "123 Main Street, Springfield, IL 62701".into(),
            },
            39.7817,
            -89.6501,
        );
        builder.add(
            Address {
                house_number: Some("456".into()),
                street: Some("Oak Avenue".into()),
                city: Some("Portland".into()),
                state: Some("OR".into()),
                postcode: Some("97201".into()),
                country: Some("US".into()),
                full: "456 Oak Avenue, Portland, OR 97201".into(),
            },
            45.5152,
            -122.6784,
        );
        builder.build().unwrap()
    }

    #[test]
    fn test_batch_process() {
        let geocoder = test_geocoder();
        let batch = BatchGeocoder::new(&geocoder, 5);
        let queries = vec!["Main Street", "Oak Avenue", "Nonexistent Place"];
        let (results, summary) = batch.process(&queries);

        assert_eq!(results.len(), 3);
        assert_eq!(summary.total, 3);
    }

    #[test]
    fn test_batch_progress() {
        let progress = BatchProgress::new(100);
        assert_eq!(progress.fraction(), 0.0);
        progress.increment();
        assert_eq!(progress.completed(), 1);
        assert!((progress.fraction() - 0.01).abs() < 1e-10);
    }

    #[test]
    fn test_batch_summary() {
        let results = vec![
            BatchResult {
                index: 0,
                query: "test".into(),
                results: vec![],
                status: BatchStatus::NoMatch,
            },
            BatchResult {
                index: 1,
                query: "test2".into(),
                results: vec![],
                status: BatchStatus::NoMatch,
            },
        ];
        let summary = compute_summary(&results);
        assert_eq!(summary.total, 2);
        assert_eq!(summary.no_match, 2);
        assert_eq!(summary.matched, 0);
    }
}
