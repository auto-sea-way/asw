use asw_core::routing::compute_route;
use axum::{
    extract::{Query, Request, State},
    http::StatusCode,
    middleware::{self, Next},
    response::Json,
    routing::get,
    Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use subtle::ConstantTimeEq;

use crate::state::ServerState;

#[derive(Deserialize)]
pub struct RouteQuery {
    /// "lat,lon"
    pub from: String,
    /// "lat,lon"
    pub to: String,
}

#[derive(Serialize)]
pub struct RouteResponse {
    pub distance_nm: f64,
    pub raw_hops: usize,
    pub smooth_hops: usize,
    pub geometry: serde_json::Value,
}

#[derive(Serialize)]
pub struct ErrorResponse {
    pub error: String,
}

#[derive(Serialize)]
pub struct InfoResponse {
    pub nodes: u32,
    pub edges: u32,
    pub graph_path: String,
    pub version: String,
}

fn parse_latlng(s: &str) -> Option<(f64, f64)> {
    let (lat_s, lon_s) = s.split_once(',')?;
    if lon_s.contains(',') {
        return None;
    }
    let lat: f64 = lat_s.trim().parse().ok()?;
    let lon: f64 = lon_s.trim().parse().ok()?;
    Some((lat, lon))
}

async fn route_handler(
    State(state): State<Arc<ServerState>>,
    Query(params): Query<RouteQuery>,
) -> Result<Json<RouteResponse>, (StatusCode, Json<ErrorResponse>)> {
    let guard = state.inner.read().await;
    let app = guard.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ErrorResponse {
                error: "Server is loading graph...".into(),
            }),
        )
    })?;

    let (from_lat, from_lon) = parse_latlng(&params.from).ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Invalid 'from' parameter. Expected: lat,lon".into(),
            }),
        )
    })?;

    let (to_lat, to_lon) = parse_latlng(&params.to).ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Invalid 'to' parameter. Expected: lat,lon".into(),
            }),
        )
    })?;

    let knn = |lat: f64, lon: f64| -> Option<(u32, f64)> { app.nearest_node(lat, lon) };

    let result = app
        .with_astar_buffers(|buffers| {
            compute_route(
                &app.graph,
                from_lat,
                from_lon,
                to_lat,
                to_lon,
                &app.coastline,
                &knn,
                buffers,
            )
        })
        .await
        .ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "No route found between the given points".into(),
            }),
        )
    })?;

    let geometry = serde_json::json!({
        "type": "LineString",
        "coordinates": result.coordinates
    });

    Ok(Json(RouteResponse {
        distance_nm: (result.distance_nm * 10.0).round() / 10.0,
        raw_hops: result.raw_hops,
        smooth_hops: result.smooth_hops,
        geometry,
    }))
}

async fn health_handler() -> &'static str {
    "ok"
}

async fn ready_handler(State(state): State<Arc<ServerState>>) -> Result<&'static str, StatusCode> {
    let guard = state.inner.read().await;
    if guard.is_some() {
        Ok("ready")
    } else {
        Err(StatusCode::SERVICE_UNAVAILABLE)
    }
}

async fn info_handler(
    State(state): State<Arc<ServerState>>,
) -> Result<Json<InfoResponse>, StatusCode> {
    let guard = state.inner.read().await;
    let app = guard.as_ref().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;

    Ok(Json(InfoResponse {
        nodes: app.graph.num_nodes,
        edges: app.graph.num_edges,
        graph_path: state.graph_path.clone(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    }))
}

async fn api_key_middleware(
    State(state): State<Arc<ServerState>>,
    req: Request,
    next: Next,
) -> Result<axum::response::Response, (StatusCode, Json<ErrorResponse>)> {
    let provided = req.headers().get("X-Api-Key").and_then(|v| v.to_str().ok());

    match provided {
        Some(key) if key.as_bytes().ct_eq(state.api_key().as_bytes()).into() => {
            Ok(next.run(req).await)
        }
        _ => {
            tracing::warn!("Rejected request: invalid or missing API key");
            Err((
                StatusCode::UNAUTHORIZED,
                Json(ErrorResponse {
                    error: "unauthorized".into(),
                }),
            ))
        }
    }
}

pub fn create_router(state: Arc<ServerState>) -> Router {
    let protected = Router::new()
        .route("/route", get(route_handler))
        .route("/info", get(info_handler))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            api_key_middleware,
        ));

    Router::new()
        .route("/health", get(health_handler))
        .route("/ready", get(ready_handler))
        .merge(protected)
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid() {
        let (lat, lon) = parse_latlng("36.85,28.27").unwrap();
        assert!((lat - 36.85).abs() < 1e-10);
        assert!((lon - 28.27).abs() < 1e-10);
    }

    #[test]
    fn parse_whitespace() {
        let (lat, lon) = parse_latlng(" 36.85 , 28.27 ").unwrap();
        assert!((lat - 36.85).abs() < 1e-10);
        assert!((lon - 28.27).abs() < 1e-10);
    }

    #[test]
    fn parse_negative() {
        let (lat, lon) = parse_latlng("-33.9,18.4").unwrap();
        assert!((lat - -33.9).abs() < 1e-10);
        assert!((lon - 18.4).abs() < 1e-10);
    }

    #[test]
    fn parse_invalid() {
        assert!(parse_latlng("abc,def").is_none());
    }

    #[test]
    fn parse_too_many_commas() {
        assert!(parse_latlng("1,2,3").is_none());
    }

    #[test]
    fn parse_empty() {
        assert!(parse_latlng("").is_none());
    }

    use axum::body::Body;
    use axum::http::{Request, StatusCode as HyperStatus};
    use tower::ServiceExt;

    fn test_state() -> Arc<ServerState> {
        Arc::new(ServerState::new(
            "test.graph".into(),
            "secret-key-1234567890".into(),
        ))
    }

    #[tokio::test]
    async fn health_no_auth_required() {
        let app = create_router(test_state());
        let req = Request::get("/health").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), HyperStatus::OK);
    }

    #[tokio::test]
    async fn ready_no_auth_required() {
        let app = create_router(test_state());
        let req = Request::get("/ready").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), HyperStatus::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn route_returns_401_without_key() {
        let app = create_router(test_state());
        let req = Request::get("/route?from=36.85,28.27&to=36.90,28.30")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), HyperStatus::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn info_returns_401_without_key() {
        let app = create_router(test_state());
        let req = Request::get("/info").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), HyperStatus::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn route_returns_401_with_wrong_key() {
        let app = create_router(test_state());
        let req = Request::get("/route?from=36.85,28.27&to=36.90,28.30")
            .header("X-Api-Key", "wrong-key")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), HyperStatus::UNAUTHORIZED);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json, serde_json::json!({"error": "unauthorized"}));
    }

    #[tokio::test]
    async fn info_returns_401_with_wrong_key() {
        let app = create_router(test_state());
        let req = Request::get("/info")
            .header("X-Api-Key", "wrong-key")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), HyperStatus::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn route_passes_with_correct_key() {
        let app = create_router(test_state());
        let req = Request::get("/route?from=36.85,28.27&to=36.90,28.30")
            .header("X-Api-Key", "secret-key-1234567890")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), HyperStatus::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn info_passes_with_correct_key() {
        let app = create_router(test_state());
        let req = Request::get("/info")
            .header("X-Api-Key", "secret-key-1234567890")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), HyperStatus::SERVICE_UNAVAILABLE);
    }
}
