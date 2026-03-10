use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::Json,
    routing::get,
    Router,
};
use serde::{Deserialize, Serialize};
use asw_core::routing::compute_route;
use std::sync::Arc;

use crate::state::AppState;

#[derive(Deserialize)]
pub struct RouteQuery {
    /// "lat,lon"
    pub from: String,
    /// "lat,lon"
    pub to: String,
}

#[derive(Serialize)]
pub struct RouteResponse {
    pub distance_km: f64,
    pub raw_hops: usize,
    pub smooth_hops: usize,
    pub geometry: serde_json::Value,
}

#[derive(Serialize)]
pub struct ErrorResponse {
    pub error: String,
}

fn parse_latlng(s: &str) -> Option<(f64, f64)> {
    let parts: Vec<&str> = s.split(',').collect();
    if parts.len() != 2 {
        return None;
    }
    let lat: f64 = parts[0].trim().parse().ok()?;
    let lon: f64 = parts[1].trim().parse().ok()?;
    Some((lat, lon))
}

async fn route_handler(
    State(state): State<Arc<AppState>>,
    Query(params): Query<RouteQuery>,
) -> Result<Json<RouteResponse>, (StatusCode, Json<ErrorResponse>)> {
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

    let knn = |lat: f64, lon: f64| -> Option<(u32, f64)> { state.nearest_node(lat, lon) };

    let result = compute_route(
        &state.graph,
        from_lat,
        from_lon,
        to_lat,
        to_lon,
        &state.coastline,
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
        distance_km: (result.distance_km * 10.0).round() / 10.0,
        raw_hops: result.raw_hops,
        smooth_hops: result.smooth_hops,
        geometry,
    }))
}

async fn health_handler() -> &'static str {
    "ok"
}

pub fn create_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/route", get(route_handler))
        .route("/health", get(health_handler))
        .with_state(state)
}
