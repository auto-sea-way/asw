use asw_core::graph::RoutingGraph;
use std::sync::Arc;

/// Wrapper that tracks readiness — the HTTP server starts before the graph is loaded.
///
/// `app` is set once by the graph loader; handlers clone the `Arc` out and run
/// CPU-bound work on a `spawn_blocking` thread without holding any lock.
pub struct ServerState {
    pub app: std::sync::OnceLock<Arc<AppState>>,
    pub graph_path: String,
    pub(crate) api_key: String,
    /// Bounds the number of `/route` requests concurrently running A* search
    /// to the A* buffer pool's capacity (`asw_core::astar_pool::DEFAULT_POOL_SIZE`).
    ///
    /// Without this, `spawn_blocking` (used to run route computation off the
    /// async executor) can spin up to Tokio's 512-thread blocking pool, and
    /// each thread beyond the pool's `DEFAULT_POOL_SIZE` buffer sets forces
    /// `AstarPool::acquire` to allocate a fresh full-size buffer set
    /// (hundreds of MB at planet scale) — a handful of concurrent long routes
    /// can OOM a small instance. Acquiring this permit is `async`, so
    /// requests beyond the limit queue on the `.await` point without
    /// occupying a blocking-pool thread; see `api::route_handler`.
    pub route_permits: tokio::sync::Semaphore,
}

impl ServerState {
    pub fn new(graph_path: String, api_key: String) -> Self {
        assert!(
            !api_key.trim().is_empty(),
            "API key must not be empty or whitespace-only"
        );
        Self {
            app: std::sync::OnceLock::new(),
            graph_path,
            api_key,
            route_permits: tokio::sync::Semaphore::new(asw_core::astar_pool::DEFAULT_POOL_SIZE),
        }
    }
}

/// Shared application state for the HTTP server.
pub struct AppState {
    pub graph: RoutingGraph,
    /// A* search buffer pool, filled lazily on first use up to
    /// `asw_core::astar_pool::DEFAULT_POOL_SIZE` buffer sets; concurrent
    /// access above that capacity is prevented upstream by
    /// `ServerState::route_permits`, a semaphore sized to match, so requests
    /// beyond the pool's capacity queue instead of forcing it to grow.
    pub(crate) astar_pool: asw_core::astar_pool::AstarPool,
}

impl AppState {
    /// Build AppState from an opened RoutingGraph. The coastline index is a
    /// view over the mapped file, so nothing is built here.
    pub fn new(graph: RoutingGraph) -> Self {
        let astar_pool = asw_core::astar_pool::AstarPool::new(
            graph.num_nodes() as usize,
            asw_core::astar_pool::DEFAULT_POOL_SIZE,
        );
        Self { graph, astar_pool }
    }
}
