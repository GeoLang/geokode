pub mod auth;
pub mod metrics;

use axum::{
    Json, Router,
    extract::{
        DefaultBodyLimit, Query, State, rejection::JsonRejection, rejection::QueryRejection,
    },
    http::StatusCode,
    middleware,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use geokode_core::address::GeoResult;
use geokode_core::geocode::{Geocoder, Point};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing::info;

pub type AppState = Arc<Geocoder>;

const MAX_QUERY_CHARS: usize = 256;
const DEFAULT_LIMIT: usize = 5;
const MAX_LIMIT: usize = 50;
const BATCH_DEFAULT_LIMIT: usize = 1;
const BATCH_MAX_LIMIT: usize = 5;
const BATCH_MAX_QUERIES: usize = 100;
const BATCH_MAX_BODY_BYTES: usize = 64 * 1024;

pub fn create_router(geocoder: Geocoder) -> Router {
    let state: AppState = Arc::new(geocoder);

    metrics::install();

    Router::new()
        .route("/forward", get(forward_handler))
        .route("/reverse", get(reverse_handler))
        .route("/autocomplete", get(autocomplete_handler))
        .route(
            "/batch",
            post(batch_handler).layer(DefaultBodyLimit::max(BATCH_MAX_BODY_BYTES)),
        )
        .route("/health", get(health_handler))
        .route("/healthz", get(liveness_handler))
        .route("/readyz", get(readiness_handler))
        .route("/metrics", get(metrics::metrics_handler))
        .layer(middleware::from_fn(auth::auth_middleware))
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
        .with_state(state)
}

pub fn init_tracing() {
    use tracing_subscriber::{EnvFilter, fmt};
    fmt()
        .with_env_filter(
            EnvFilter::from_default_env().add_directive("geokode=info".parse().unwrap()),
        )
        .init();
    info!("tracing initialised");
}

#[derive(Debug)]
pub struct BadRequest(String);

#[derive(Serialize)]
struct ErrorBody {
    error: String,
}

impl IntoResponse for BadRequest {
    fn into_response(self) -> Response {
        (StatusCode::BAD_REQUEST, Json(ErrorBody { error: self.0 })).into_response()
    }
}

impl From<QueryRejection> for BadRequest {
    fn from(rejection: QueryRejection) -> Self {
        BadRequest(format!("The query string could not be read: {rejection}."))
    }
}

impl From<JsonRejection> for BadRequest {
    fn from(rejection: JsonRejection) -> Self {
        if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
            return BadRequest(format!(
                "The request body is larger than {} KiB.",
                BATCH_MAX_BODY_BYTES / 1024
            ));
        }
        BadRequest(format!("The request body could not be read: {rejection}."))
    }
}

// strings so a malformed number answers with our own 400 message
#[derive(Deserialize)]
pub struct SearchParams {
    q: Option<String>,
    limit: Option<String>,
    lat: Option<String>,
    lon: Option<String>,
}

#[derive(Deserialize)]
pub struct ReverseParams {
    lat: Option<String>,
    lon: Option<String>,
    limit: Option<String>,
}

#[derive(Deserialize)]
pub struct BatchRequest {
    queries: Vec<String>,
    limit: Option<i64>,
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

struct Search {
    text: String,
    limit: usize,
    bias: Option<Point>,
}

fn query_text(raw: Option<&str>, field: &str) -> Result<String, BadRequest> {
    let text = raw.unwrap_or_default().trim();
    let chars = text.chars().count();
    if chars == 0 {
        return Err(BadRequest(format!("The {field} must not be empty.")));
    }
    if chars > MAX_QUERY_CHARS {
        return Err(BadRequest(format!(
            "The {field} is longer than {MAX_QUERY_CHARS} characters."
        )));
    }
    Ok(text.to_string())
}

fn limit_within(value: i64, max: usize) -> Result<usize, BadRequest> {
    usize::try_from(value)
        .ok()
        .filter(|limit| (1..=max).contains(limit))
        .ok_or_else(|| BadRequest(format!("The limit must be between 1 and {max}.")))
}

fn parse_limit(raw: Option<&str>, default: usize, max: usize) -> Result<usize, BadRequest> {
    let Some(raw) = raw else {
        return Ok(default);
    };
    let value: i64 = raw
        .trim()
        .parse()
        .map_err(|_| BadRequest(format!("The limit must be a whole number, not {raw:?}.")))?;
    limit_within(value, max)
}

fn parse_coordinate(raw: &str, name: &str, bound: f64) -> Result<f64, BadRequest> {
    raw.trim()
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && value.abs() <= bound)
        .ok_or_else(|| {
            BadRequest(format!(
                "The {name} must be a number between -{bound} and {bound}."
            ))
        })
}

const MAX_LATITUDE: f64 = 90.0;
const MAX_LONGITUDE: f64 = 180.0;

fn parse_point(lat: &str, lon: &str) -> Result<Point, BadRequest> {
    Ok(Point {
        lat: parse_coordinate(lat, "lat", MAX_LATITUDE)?,
        lon: parse_coordinate(lon, "lon", MAX_LONGITUDE)?,
    })
}

fn parse_search(params: &SearchParams) -> Result<Search, BadRequest> {
    let bias = match (params.lat.as_deref(), params.lon.as_deref()) {
        (None, None) => None,
        (Some(lat), Some(lon)) => Some(parse_point(lat, lon)?),
        _ => return Err(BadRequest("Give both lat and lon, or neither.".to_string())),
    };
    Ok(Search {
        text: query_text(params.q.as_deref(), "query q")?,
        limit: parse_limit(params.limit.as_deref(), DEFAULT_LIMIT, MAX_LIMIT)?,
        bias,
    })
}

async fn forward_handler(
    State(geocoder): State<AppState>,
    params: Result<Query<SearchParams>, QueryRejection>,
) -> Result<Json<ApiResponse>, BadRequest> {
    ::metrics::counter!("geokode_forward_requests").increment(1);
    let Query(params) = params?;
    let search = parse_search(&params)?;
    let results = geocoder.forward(&search.text, search.limit, search.bias);
    Ok(Json(ApiResponse { results }))
}

async fn autocomplete_handler(
    State(geocoder): State<AppState>,
    params: Result<Query<SearchParams>, QueryRejection>,
) -> Result<Json<ApiResponse>, BadRequest> {
    ::metrics::counter!("geokode_autocomplete_requests").increment(1);
    let Query(params) = params?;
    let search = parse_search(&params)?;
    let results = geocoder.autocomplete(&search.text, search.limit, search.bias);
    Ok(Json(ApiResponse { results }))
}

async fn reverse_handler(
    State(geocoder): State<AppState>,
    params: Result<Query<ReverseParams>, QueryRejection>,
) -> Result<Json<ApiResponse>, BadRequest> {
    ::metrics::counter!("geokode_reverse_requests").increment(1);
    let params = params?;
    let (Some(lat), Some(lon)) = (params.lat.as_deref(), params.lon.as_deref()) else {
        return Err(BadRequest("Both lat and lon are required.".to_string()));
    };
    let point = parse_point(lat, lon)?;
    let limit = parse_limit(params.limit.as_deref(), DEFAULT_LIMIT, MAX_LIMIT)?;
    let results = geocoder.reverse(point.lon, point.lat, limit);
    Ok(Json(ApiResponse { results }))
}

async fn batch_handler(
    State(geocoder): State<AppState>,
    body: Result<Json<BatchRequest>, JsonRejection>,
) -> Result<Json<BatchResponse>, BadRequest> {
    ::metrics::counter!("geokode_batch_requests").increment(1);
    let Json(body) = body?;
    if body.queries.is_empty() || body.queries.len() > BATCH_MAX_QUERIES {
        return Err(BadRequest(format!(
            "Send between 1 and {BATCH_MAX_QUERIES} queries."
        )));
    }
    let limit = match body.limit {
        Some(value) => limit_within(value, BATCH_MAX_LIMIT)?,
        None => BATCH_DEFAULT_LIMIT,
    };
    let texts = body
        .queries
        .iter()
        .map(|query| query_text(Some(query), "each query"))
        .collect::<Result<Vec<_>, _>>()?;
    let results = texts
        .iter()
        .map(|text| geocoder.forward(text, limit, None))
        .collect();
    Ok(Json(BatchResponse { results }))
}

async fn health_handler(State(geocoder): State<AppState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        records: geocoder.len(),
    })
}

async fn liveness_handler() -> &'static str {
    "ok"
}

async fn readiness_handler(State(geocoder): State<AppState>) -> (StatusCode, &'static str) {
    if !geocoder.is_empty() {
        (StatusCode::OK, "ready")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "not ready")
    }
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

    async fn send(request: Request<Body>) -> (StatusCode, serde_json::Value) {
        let response = create_router(test_geocoder())
            .oneshot(request)
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, body)
    }

    async fn get_json(uri: &str) -> (StatusCode, serde_json::Value) {
        send(Request::builder().uri(uri).body(Body::empty()).unwrap()).await
    }

    async fn post_batch(body: String) -> (StatusCode, serde_json::Value) {
        send(
            Request::builder()
                .method("POST")
                .uri("/batch")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
    }

    async fn assert_bad_request(uri: &str) {
        let (status, body) = get_json(uri).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}");
        assert!(
            body["error"].as_str().is_some_and(|e| !e.is_empty()),
            "{uri}: {body}"
        );
    }

    #[tokio::test]
    async fn forward_endpoint_flags_fuzzy_matches() {
        let (_, body) = get_json("/forward?q=Spirngfield").await;
        assert_eq!(body["results"][0]["match_type"], "fuzzy", "got {body}");
    }

    #[tokio::test]
    async fn every_result_field_is_present() {
        let (status, body) = get_json("/forward?q=123+Main+St").await;
        assert_eq!(status, StatusCode::OK);
        let result = body["results"][0].as_object().unwrap();
        for field in [
            "name",
            "display_name",
            "address",
            "country_code",
            "lat",
            "lon",
            "bbox",
            "kind",
            "osm_type",
            "osm_id",
            "osm_key",
            "osm_value",
            "admin_level",
            "population",
            "confidence",
            "match_type",
        ] {
            assert!(result.contains_key(field), "missing {field} in {body}");
        }
    }

    #[tokio::test]
    async fn search_caps_answer_400() {
        let long = "a".repeat(MAX_QUERY_CHARS + 1);
        for endpoint in ["/forward", "/autocomplete"] {
            for query in [
                String::new(),
                "q=".to_string(),
                "q=%20%20".to_string(),
                format!("q={long}"),
                "q=main&limit=0".to_string(),
                "q=main&limit=51".to_string(),
                "q=main&limit=five".to_string(),
                "q=main&lat=39.7".to_string(),
                "q=main&lon=-89.6".to_string(),
                "q=main&lat=91&lon=0".to_string(),
                "q=main&lat=0&lon=181".to_string(),
                "q=main&lat=north&lon=0".to_string(),
            ] {
                assert_bad_request(&format!("{endpoint}?{query}")).await;
            }
        }
    }

    #[tokio::test]
    async fn search_accepts_the_edges_of_each_cap() {
        let longest = "a".repeat(MAX_QUERY_CHARS);
        for uri in [
            format!("/forward?q={longest}"),
            "/forward?q=main&limit=50&lat=-90&lon=180".to_string(),
            "/autocomplete?q=m&limit=1".to_string(),
        ] {
            let (status, body) = get_json(&uri).await;
            assert_eq!(status, StatusCode::OK, "{uri}: {body}");
        }
    }

    #[tokio::test]
    async fn forward_returns_at_most_limit_results() {
        let (_, body) = get_json("/forward?q=main&limit=1").await;
        assert!(body["results"].as_array().unwrap().len() <= 1);
    }

    #[tokio::test]
    async fn reverse_caps_answer_400() {
        for query in [
            "",
            "lat=39.78",
            "lon=-89.65",
            "lat=95&lon=-89.65",
            "lat=39.78&lon=-189",
            "lat=39.78&lon=-89.65&limit=0",
            "lat=39.78&lon=-89.65&limit=51",
        ] {
            assert_bad_request(&format!("/reverse?{query}")).await;
        }
        let (status, _) = get_json("/reverse?lat=39.78&lon=-89.65&limit=50").await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn batch_answers_in_query_order() {
        let (status, body) =
            post_batch(r#"{"queries": ["123 Main St", "zzqqwx flurbleglop"]}"#.to_string()).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let results = body["results"].as_array().unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].as_array().unwrap().len(), 1);
        assert!(results[1].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn batch_caps_answer_400() {
        let too_many = serde_json::json!({ "queries": vec!["main"; BATCH_MAX_QUERIES + 1] });
        let too_long = serde_json::json!({ "queries": ["a".repeat(MAX_QUERY_CHARS + 1)] });
        let oversized = serde_json::json!({
            "queries": vec!["a".repeat(MAX_QUERY_CHARS); BATCH_MAX_QUERIES],
            "padding": "x".repeat(BATCH_MAX_BODY_BYTES),
        });
        for body in [
            r#"{"queries": []}"#.to_string(),
            r#"{"queries": [""]}"#.to_string(),
            r#"{"queries": ["main"], "limit": 0}"#.to_string(),
            r#"{"queries": ["main"], "limit": 6}"#.to_string(),
            r#"{"queries": "main"}"#.to_string(),
            "not json".to_string(),
            too_many.to_string(),
            too_long.to_string(),
            oversized.to_string(),
        ] {
            let (status, response) = post_batch(body.clone()).await;
            assert_eq!(
                status,
                StatusCode::BAD_REQUEST,
                "{}",
                &body[..body.len().min(80)]
            );
            assert!(response["error"].is_string(), "{response}");
        }
    }

    #[tokio::test]
    async fn batch_limit_caps_each_answer() {
        let (_, body) = post_batch(r#"{"queries": ["main"], "limit": 5}"#.to_string()).await;
        assert!(body["results"][0].as_array().unwrap().len() <= 5);
    }

    #[tokio::test]
    async fn health_endpoint() {
        let (status, body) = get_json("/health").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["records"], 1);
    }

    #[tokio::test]
    async fn metrics_endpoint() {
        let response = create_router(test_geocoder())
            .oneshot(
                Request::builder()
                    .uri("/metrics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn liveness_probe() {
        let response = create_router(test_geocoder())
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}
