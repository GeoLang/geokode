//! Prometheus metrics endpoint.

use axum::response::IntoResponse;
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use std::sync::OnceLock;

static PROMETHEUS_HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();

/// Install the Prometheus metrics recorder. Call once at startup.
/// Safe to call multiple times — subsequent calls are no-ops.
pub fn install() {
    PROMETHEUS_HANDLE.get_or_init(|| {
        let handle = PrometheusBuilder::new()
            .install_recorder()
            .expect("failed to install Prometheus recorder");

        metrics::describe_counter!("geokode_requests_total", "Total HTTP requests");
        metrics::describe_counter!("geokode_forward_requests", "Forward geocode requests");
        metrics::describe_counter!("geokode_reverse_requests", "Reverse geocode requests");
        metrics::describe_counter!("geokode_autocomplete_requests", "Autocomplete requests");
        metrics::describe_counter!("geokode_batch_requests", "Batch geocode requests");
        metrics::describe_histogram!(
            "geokode_request_duration_seconds",
            "Request duration in seconds"
        );
        metrics::describe_gauge!("geokode_index_records", "Number of indexed records");

        handle
    });
}

/// Handler for GET /metrics — serves Prometheus text format.
pub async fn metrics_handler() -> impl IntoResponse {
    let output = match PROMETHEUS_HANDLE.get() {
        Some(handle) => handle.render(),
        None => "# HELP geokode_up Server is running\ngeokode_up 1\n".to_string(),
    };
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        output,
    )
}
