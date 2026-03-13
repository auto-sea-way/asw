use asw_core::routing::compute_route;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::Json,
    routing::get,
    Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

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

    let result = compute_route(
        &app.graph,
        from_lat,
        from_lon,
        to_lat,
        to_lon,
        &app.coastline,
        &knn,
    )
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

pub fn create_router(state: Arc<ServerState>) -> Router {
    Router::new()
        .route("/route", get(route_handler))
        .route("/health", get(health_handler))
        .route("/ready", get(ready_handler))
        .route("/info", get(info_handler))
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
}
