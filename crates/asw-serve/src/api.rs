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
use tower_http::cors::{Any, CorsLayer};

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
    // Clone the Arc<AppState> out and drop the read guard immediately —
    // holding it across the (potentially slow, CPU-bound) route computation
    // below would starve other readers of `state.inner`, including the
    // `/health`/`/ready` probes and concurrent `/route`/`/info` requests.
    let app = state.app().await.ok_or_else(|| {
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

    // Bound concurrent A* computations to the buffer pool's capacity before
    // handing off to the blocking pool: without this, `spawn_blocking` can
    // spin up to Tokio's 512-thread blocking pool, and each thread beyond the
    // pool's pre-allocated buffer sets forces `AstarPool::acquire` to
    // allocate a fresh full-size buffer set (hundreds of MB at planet scale),
    // which can OOM a small instance under a handful of concurrent long
    // routes (see finding 1 in the 2026-07-06 project review). Acquiring is
    // `async`, so requests beyond the limit queue here instead of consuming a
    // blocking-pool thread. `acquire()` only errs if the semaphore has been
    // closed, which this server never does — map defensively to 503 rather
    // than panicking via `unwrap`/`expect`.
    let _permit = state.route_permits.acquire().await.map_err(|_| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ErrorResponse {
                error: "Server is at capacity, please retry".into(),
            }),
        )
    })?;

    // Run nearest-node snapping + A* + smoothing on a blocking-pool thread:
    // this is CPU-bound work (tens of ms to >1s for long routes) that must
    // not occupy a tokio worker thread, or it starves other tasks scheduled
    // on the same worker (see finding 6 in the 2026-07-06 project review).
    let result = tokio::task::spawn_blocking(move || {
        let knn = |lat: f64, lon: f64| -> Option<(u32, f64)> { app.nearest_node(lat, lon) };
        app.with_astar_buffers(|buffers| {
            compute_route(
                &app.graph,
                from_lat,
                from_lon,
                to_lat,
                to_lon,
                &app.coastline,
                &knn,
                buffers,
                0.0,
            )
        })
    })
    .await
    .map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "Route computation failed unexpectedly".into(),
            }),
        )
    })?
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
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([axum::http::Method::GET])
        .allow_headers(Any);

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
        .layer(cors)
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

    /// Directly install an `AppState` as "ready", bypassing `set_ready`.
    ///
    /// `set_ready` uses `blocking_write()` because production code calls it
    /// from inside `spawn_blocking` (see `main.rs`); calling it directly from
    /// an async test body panics ("Cannot block the current thread from
    /// within a runtime"). Tests instead populate `inner` via the async
    /// `write()` guard, which is equivalent for our purposes.
    async fn mark_ready(state: &ServerState, app: crate::state::AppState) {
        *state.inner.write().await = Some(Arc::new(app));
    }

    /// Build a `ServerState` whose graph has already finished loading,
    /// backed by a tiny 3-node chain graph. Used to exercise the `/route`
    /// happy path end-to-end through the new `Arc<AppState>` +
    /// `spawn_blocking` pipeline (finding 6), rather than only the "still
    /// loading" 503 path the other tests cover.
    async fn ready_state_with_small_graph() -> Arc<ServerState> {
        use crate::state::AppState;
        use asw_core::graph::GraphBuilder;

        let coords = [(36.848, 28.268), (36.9, 28.3), (37.0, 28.5)];
        let mut entries: Vec<(u64, f64, f64)> = coords
            .iter()
            .map(|&(lat, lng)| {
                let cell = h3o::LatLng::new(lat, lng)
                    .unwrap()
                    .to_cell(h3o::Resolution::Five);
                (u64::from(cell), lat, lng)
            })
            .collect();
        entries.sort_by_key(|(h3, _, _)| *h3);
        entries.dedup_by_key(|(h3, _, _)| *h3);

        let mut b = GraphBuilder::new();
        let mut ids = Vec::new();
        for &(h3, lat, lng) in &entries {
            ids.push(b.add_node(h3, lat, lng, 255));
        }
        for i in 0..ids.len().saturating_sub(1) {
            b.add_edge(ids[i], ids[i + 1], 1.0);
        }
        let graph = b.build();

        let state = Arc::new(ServerState::new(
            "test.graph".into(),
            "secret-key-1234567890".into(),
        ));
        mark_ready(&state, AppState::new(graph)).await;
        state
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

    /// End-to-end happy path once the graph is loaded: exercises the full
    /// `state.app()` clone -> `spawn_blocking` -> `nearest_node` ->
    /// `compute_route` -> `with_astar_buffers` pipeline introduced for
    /// finding 6, verifying it still produces a correct route (not just
    /// that the code compiles / doesn't deadlock).
    #[tokio::test]
    async fn route_returns_200_with_valid_route_once_ready() {
        let app = create_router(ready_state_with_small_graph().await);
        let req = Request::get("/route?from=36.848,28.268&to=37.0,28.5")
            .header("X-Api-Key", "secret-key-1234567890")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), HyperStatus::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(
            json["distance_nm"].as_f64().unwrap() > 0.0,
            "expected a positive route distance, got {json:?}"
        );
        assert!(json["raw_hops"].as_u64().unwrap() >= 1);
        assert_eq!(json["geometry"]["type"], "LineString");
    }

    /// More concurrent `/route` requests than the A* buffer pool holds
    /// (`DEFAULT_POOL_SIZE` buffer sets) must all still complete successfully
    /// — the `route_permits` semaphore queues the surplus request(s) rather
    /// than letting them fail, panic, or force the pool to allocate beyond
    /// its capacity (finding 1, 2026-07-06 review).
    #[tokio::test]
    async fn concurrent_route_requests_beyond_pool_capacity_all_succeed() {
        let state = ready_state_with_small_graph().await;
        let app = create_router(state);

        let concurrency = asw_core::astar_pool::DEFAULT_POOL_SIZE + 1;
        let mut handles = Vec::with_capacity(concurrency);
        for _ in 0..concurrency {
            let app = app.clone();
            handles.push(tokio::spawn(async move {
                let req = Request::get("/route?from=36.848,28.268&to=37.0,28.5")
                    .header("X-Api-Key", "secret-key-1234567890")
                    .body(Body::empty())
                    .unwrap();
                app.oneshot(req).await.unwrap()
            }));
        }

        for handle in handles {
            let resp = handle.await.expect("request task must not panic");
            assert_eq!(
                resp.status(),
                HyperStatus::OK,
                "every request must complete successfully, queueing rather than failing"
            );
        }
    }

    /// A query point with no reachable node (empty ready graph) must still
    /// surface as 404, not hang or panic, through the spawn_blocking path.
    #[tokio::test]
    async fn route_returns_404_when_no_route_found_once_ready() {
        use crate::state::AppState;
        use asw_core::graph::GraphBuilder;

        let state = Arc::new(ServerState::new(
            "test.graph".into(),
            "secret-key-1234567890".into(),
        ));
        mark_ready(&state, AppState::new(GraphBuilder::new().build())).await;

        let app = create_router(state);
        let req = Request::get("/route?from=36.848,28.268&to=37.0,28.5")
            .header("X-Api-Key", "secret-key-1234567890")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), HyperStatus::NOT_FOUND);
    }

    #[tokio::test]
    async fn health_includes_cors_header() {
        let app = create_router(test_state());
        let req = Request::get("/health")
            .header("Origin", "http://localhost:8080")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), HyperStatus::OK);
        assert!(
            resp.headers().contains_key("access-control-allow-origin"),
            "expected Access-Control-Allow-Origin header on response"
        );
    }

    #[tokio::test]
    async fn preflight_allows_api_key_header() {
        let app = create_router(test_state());
        let req = Request::builder()
            .method("OPTIONS")
            .uri("/route")
            .header("Origin", "http://localhost:8080")
            .header("Access-Control-Request-Method", "GET")
            .header("Access-Control-Request-Headers", "x-api-key")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), HyperStatus::OK);
        assert!(
            resp.headers().contains_key("access-control-allow-headers"),
            "expected Access-Control-Allow-Headers on preflight response"
        );
    }
}
