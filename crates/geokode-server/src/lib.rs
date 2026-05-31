//! # geokode-server
//!
//! REST API server for geocoding operations.
//! Endpoints: /forward, /reverse, /autocomplete, /batch

use axum::{
    Json, Router,
    extract::{Query, State},
    routing::{get, post},
};
use geokode_core::address::GeoResult;
use geokode_core::geocode::Geocoder;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing::info;

/// Shared application state.
pub type AppState = Arc<Geocoder>;

/// Create the API router.
pub fn create_router(geocoder: Geocoder) -> Router {
    let state: AppState = Arc::new(geocoder);

    Router::new()
        .route("/forward", get(forward_handler))
        .route("/reverse", get(reverse_handler))
        .route("/autocomplete", get(autocomplete_handler))
        .route("/batch", post(batch_handler))
        .route("/health", get(health_handler))
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
        .with_state(state)
}

/// Initialise the tracing subscriber (call once at startup).
pub fn init_tracing() {
    use tracing_subscriber::{EnvFilter, fmt};
    fmt()
        .with_env_filter(
            EnvFilter::from_default_env().add_directive("geokode=info".parse().unwrap()),
        )
        .init();
    info!("tracing initialised");
}

#[derive(Deserialize)]
pub struct ForwardQuery {
    pub q: String,
}

#[derive(Deserialize)]
pub struct ReverseQuery {
    pub lon: f64,
    pub lat: f64,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

#[derive(Deserialize)]
pub struct AutocompleteQuery {
    pub q: String,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

#[derive(Deserialize)]
pub struct BatchRequest {
    pub queries: Vec<String>,
}

#[derive(Serialize)]
pub struct ApiResponse {
    pub results: Vec<GeoResult>,
}

#[derive(Serialize)]
pub struct BatchResponse {
    pub results: Vec<Vec<GeoResult>>,
}

#[derive(Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
    pub records: usize,
}

fn default_limit() -> usize {
    5
}

async fn forward_handler(
    State(geocoder): State<AppState>,
    Query(params): Query<ForwardQuery>,
) -> Json<ApiResponse> {
    let results = geocoder.forward(&params.q);
    Json(ApiResponse { results })
}

async fn reverse_handler(
    State(geocoder): State<AppState>,
    Query(params): Query<ReverseQuery>,
) -> Json<ApiResponse> {
    let results = geocoder.reverse(params.lon, params.lat, params.limit);
    Json(ApiResponse { results })
}

async fn autocomplete_handler(
    State(geocoder): State<AppState>,
    Query(params): Query<AutocompleteQuery>,
) -> Json<ApiResponse> {
    let results = geocoder.autocomplete(&params.q, params.limit);
    Json(ApiResponse { results })
}

async fn batch_handler(
    State(geocoder): State<AppState>,
    Json(body): Json<BatchRequest>,
) -> Json<BatchResponse> {
    let queries: Vec<&str> = body.queries.iter().map(|s| s.as_str()).collect();
    let results = geocoder.batch_forward(&queries);
    Json(BatchResponse { results })
}

async fn health_handler(State(geocoder): State<AppState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        records: geocoder.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use geokode_core::address::parse_address;
    use geokode_core::geocode::GeocoderBuilder;
    use tower::ServiceExt;

    fn test_geocoder() -> Geocoder {
        let mut builder = GeocoderBuilder::new();
        builder.add(parse_address("123 Main St, Springfield, IL"), 39.78, -89.65);
        builder.build().unwrap()
    }

    #[tokio::test]
    async fn health_endpoint() {
        let app = create_router(test_geocoder());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
    }
}
