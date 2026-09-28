//! Mobile bindings for the auto-sea-way routing graph. Four calls over a
//! memory-mapped v4 file: `open`, `version`, `is_water`, `route`. Generated
//! into Swift and Kotlin by UniFFI; panics never cross the boundary.

use asw_core::astar_pool::AstarBuffers;
use asw_core::graph::RoutingGraph;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::sync::{Arc, Mutex};

uniffi::setup_scaffolding!();

#[derive(Debug, Clone, PartialEq, uniffi::Error)]
pub enum AswError {
    NotFound,
    BadFormat { message: String },
    InvalidArgument { message: String },
    NoRoute,
    Internal { message: String },
}

impl std::fmt::Display for AswError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AswError::NotFound => write!(f, "graph file not found"),
            AswError::BadFormat { message } => write!(f, "not a usable graph file: {message}"),
            AswError::InvalidArgument { message } => write!(f, "invalid argument: {message}"),
            AswError::NoRoute => write!(f, "no route between the given points"),
            AswError::Internal { message } => write!(f, "internal error: {message}"),
        }
    }
}

impl std::error::Error for AswError {}

/// Answer of `Graph::is_water`. `Unknown` only when the call could not be
/// evaluated (a panic inside the graph code or a vanished mapping).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum Water {
    Water,
    Land,
    Unknown,
}

/// A point as the app thinks of it: latitude first, degrees.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct Coordinate {
    pub lat: f64,
    pub lon: f64,
}

/// Result of `Graph::route`. Same semantics as the HTTP `/route` response:
/// the polyline starts and ends at the requested points, `distance_nm`
/// counts water segments only, `land_legs` are indices of segments
/// (`coordinates[i] -> coordinates[i+1]`) that cross land.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct Route {
    pub coordinates: Vec<Coordinate>,
    pub distance_nm: f64,
    pub land_legs: Vec<u32>,
}

/// Test-only one-shot switch that makes the next `is_water` or `route` panic,
/// proving the panic never crosses the boundary.
#[cfg(test)]
static PANIC_NEXT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(test)]
fn maybe_panic() {
    if PANIC_NEXT.swap(false, std::sync::atomic::Ordering::SeqCst) {
        panic!("forced panic for the boundary test");
    }
}

#[cfg(not(test))]
fn maybe_panic() {}

/// An opened graph file. Reference counted across the FFI; safe to share
/// between threads. Routes serialise on the single A* buffer set.
#[derive(uniffi::Object)]
pub struct Graph {
    inner: RoutingGraph,
    buffers: Mutex<Option<AstarBuffers>>,
}

impl std::fmt::Debug for Graph {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Graph")
            .field("version", &self.inner.version())
            .field("num_nodes", &self.inner.num_nodes())
            .finish()
    }
}

/// Turn a panic payload into a message.
fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "panic".to_string()
    }
}

/// Memory-map a v4 graph file and validate its header. Milliseconds; no
/// per-node work.
#[uniffi::export]
pub fn open(path: String) -> Result<Arc<Graph>, AswError> {
    let p = Path::new(&path);
    if !p.exists() {
        return Err(AswError::NotFound);
    }
    let opened = catch_unwind(AssertUnwindSafe(|| RoutingGraph::open(p, false))).map_err(|e| {
        AswError::Internal {
            message: panic_message(e),
        }
    })?;
    let inner = opened.map_err(|e| AswError::BadFormat {
        message: format!("{e:#}"),
    })?;
    Ok(Arc::new(Graph {
        inner,
        buffers: Mutex::new(None),
    }))
}

#[uniffi::export]
impl Graph {
    /// The version string stored in the file header.
    pub fn version(&self) -> String {
        self.inner.version().to_string()
    }

    /// Is the point on navigable water? Snap to the nearest water node and
    /// count coastline crossings on the way; even means water, no node means
    /// land. Never panics.
    pub fn is_water(&self, lat: f64, lon: f64) -> Water {
        match catch_unwind(AssertUnwindSafe(|| {
            maybe_panic();
            asw_core::routing::is_water(&self.inner, lat, lon)
        })) {
            Ok(true) => Water::Water,
            Ok(false) => Water::Land,
            Err(_) => Water::Unknown,
        }
    }

    /// Route between two points with an optional shore clearance in
    /// nautical miles (0 to 5). Blocking; routes serialise on one buffer set.
    pub fn route(
        &self,
        from_lat: f64,
        from_lon: f64,
        to_lat: f64,
        to_lon: f64,
        shore_buffer_nm: f64,
    ) -> Result<Route, AswError> {
        for (name, v) in [
            ("from_lat", from_lat),
            ("from_lon", from_lon),
            ("to_lat", to_lat),
            ("to_lon", to_lon),
        ] {
            if !v.is_finite() {
                return Err(AswError::InvalidArgument {
                    message: format!("{name} is not a finite number"),
                });
            }
        }
        if !shore_buffer_nm.is_finite() || !(0.0..=5.0).contains(&shore_buffer_nm) {
            return Err(AswError::InvalidArgument {
                message: "shore_buffer_nm must be between 0 and 5 nautical miles".into(),
            });
        }
        // A panic inside must not poison the mutex for the next call: the
        // lock is taken inside the unwind boundary and dropped before it is
        // left, and a poisoned lock is recovered anyway.
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            maybe_panic();
            let mut slot = self
                .buffers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let buffers =
                slot.get_or_insert_with(|| AstarBuffers::new(self.inner.num_nodes() as usize));
            let knn = |lat: f64, lon: f64| self.inner.nearest_node(lat, lon);
            let result = asw_core::routing::compute_route(
                &self.inner,
                from_lat,
                from_lon,
                to_lat,
                to_lon,
                &self.inner.coastline(),
                &knn,
                buffers,
                shore_buffer_nm,
            );
            buffers.reset();
            result
        }));
        match outcome {
            Err(payload) => Err(AswError::Internal {
                message: panic_message(payload),
            }),
            Ok(None) => Err(AswError::NoRoute),
            Ok(Some(r)) => Ok(Route {
                coordinates: r
                    .coordinates
                    .iter()
                    .map(|c| Coordinate {
                        lat: c[1],
                        lon: c[0],
                    })
                    .collect(),
                distance_nm: r.distance_nm,
                land_legs: r.land_legs.iter().map(|&i| i as u32).collect(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use asw_core::graph::GraphBuilder;
    use std::path::PathBuf;

    /// A fresh temporary directory per call: pid plus a process-wide counter,
    /// so parallel tests never share one (a timestamp alone collides on macOS,
    /// whose clock has microsecond resolution).
    fn temp_dir(tag: &str) -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("asw-mobile-{tag}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// One res-5 water node at (36.5, 28.3), a thin mole ring between it and
    /// a berth at (36.5, 28.0), and an island ring around (36.5, 27.6).
    /// Written to a temporary directory as a v4 file.
    pub(crate) fn fixture_graph_path() -> (PathBuf, PathBuf) {
        let cell = h3o::LatLng::new(36.5, 28.3)
            .unwrap()
            .to_cell(h3o::Resolution::Five);
        let mut b = GraphBuilder::with_version("test 2026-09-28");
        b.add_node(u64::from(cell), 255);
        b.coastline_runs = vec![
            vec![
                (28.10, 36.40),
                (28.11, 36.40),
                (28.11, 36.60),
                (28.10, 36.60),
                (28.10, 36.40),
            ],
            vec![
                (27.5, 36.4),
                (27.7, 36.4),
                (27.7, 36.6),
                (27.5, 36.6),
                (27.5, 36.4),
            ],
        ];
        let dir = temp_dir("fixture");
        let path = dir.join("fixture.graph");
        b.build().save(&path).unwrap();
        (dir, path)
    }

    #[test]
    fn open_reports_the_header_version() {
        let (dir, path) = fixture_graph_path();
        let g = open(path.to_string_lossy().into_owned()).unwrap();
        assert_eq!(g.version(), "test 2026-09-28");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn open_missing_file_is_not_found() {
        let err = open("/nonexistent/asw.graph".into()).unwrap_err();
        assert_eq!(err, AswError::NotFound);
    }

    #[test]
    fn open_rejects_directory_empty_and_v3() {
        let (dir, _) = fixture_graph_path();
        let empty = dir.join("empty.graph");
        std::fs::write(&empty, b"").unwrap();
        let v3 = dir.join("v3.graph");
        std::fs::write(&v3, b"ASW\x03whatever").unwrap();
        for p in [dir.clone(), empty, v3] {
            match open(p.to_string_lossy().into_owned()) {
                Err(AswError::BadFormat { message }) => assert!(!message.is_empty()),
                other => panic!("{p:?}: expected BadFormat, got {other:?}"),
            }
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn error_display_is_readable() {
        assert_eq!(AswError::NotFound.to_string(), "graph file not found");
        assert_eq!(
            AswError::BadFormat {
                message: "x".into()
            }
            .to_string(),
            "not a usable graph file: x"
        );
    }

    #[test]
    fn is_water_marina_behind_mole_is_water() {
        let (dir, path) = fixture_graph_path();
        let g = open(path.to_string_lossy().into_owned()).unwrap();
        assert_eq!(
            g.is_water(36.5, 28.0),
            Water::Water,
            "berth behind the mole"
        );
        assert_eq!(g.is_water(36.5, 28.3), Water::Water, "next to the node");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn is_water_inside_island_is_land() {
        let (dir, path) = fixture_graph_path();
        let g = open(path.to_string_lossy().into_owned()).unwrap();
        assert_eq!(g.is_water(36.5, 27.6), Water::Land);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn is_water_far_inland_is_land() {
        // A graph with no node at all: the snapping ladder finds nothing, so
        // the answer is Land, in bounded time (the res-3 fallback disk).
        let empty = GraphBuilder::default().build();
        let dir = temp_dir("empty");
        let path = dir.join("empty.graph");
        empty.save(&path).unwrap();
        let g = open(path.to_string_lossy().into_owned()).unwrap();
        assert_eq!(g.is_water(44.8, 20.5), Water::Land);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn is_water_panic_becomes_unknown() {
        let (dir, path) = fixture_graph_path();
        let g = open(path.to_string_lossy().into_owned()).unwrap();
        PANIC_NEXT.store(true, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(g.is_water(36.5, 28.3), Water::Unknown);
        assert_eq!(
            g.is_water(36.5, 28.3),
            Water::Water,
            "the switch is one-shot"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Three res-5 nodes in a chain across the Aegean, with the coastline
    /// runs given per test.
    pub(crate) fn chain_graph_path(coast: Vec<Vec<(f64, f64)>>) -> (PathBuf, PathBuf) {
        let coords = [(36.0, 26.0), (36.5, 27.0), (37.0, 28.0)];
        let mut h3s: Vec<u64> = coords
            .iter()
            .map(|&(lat, lon)| {
                u64::from(
                    h3o::LatLng::new(lat, lon)
                        .unwrap()
                        .to_cell(h3o::Resolution::Five),
                )
            })
            .collect();
        h3s.sort_unstable();
        h3s.dedup();
        let mut b = GraphBuilder::with_version("chain");
        let ids: Vec<u32> = h3s.iter().map(|&h| b.add_node(h, 255)).collect();
        for w in ids.windows(2) {
            b.add_edge(w[0], w[1]);
        }
        b.coastline_runs = coast;
        let dir = temp_dir("chain");
        let path = dir.join("chain.graph");
        b.build().save(&path).unwrap();
        (dir, path)
    }

    #[test]
    fn route_clear_line_of_sight_returns_two_points() {
        let (dir, path) = chain_graph_path(vec![]);
        let g = open(path.to_string_lossy().into_owned()).unwrap();
        let r = g.route(36.0, 26.0, 37.0, 28.0, 0.0).unwrap();
        assert_eq!(r.coordinates.len(), 2);
        assert_eq!(
            r.coordinates[0],
            Coordinate {
                lat: 36.0,
                lon: 26.0
            }
        );
        assert_eq!(
            r.coordinates[1],
            Coordinate {
                lat: 37.0,
                lon: 28.0
            }
        );
        assert!(
            r.distance_nm > 90.0 && r.distance_nm < 130.0,
            "{}",
            r.distance_nm
        );
        assert!(r.land_legs.is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn route_through_the_graph_when_the_direct_line_is_blocked() {
        // A wall at lon 27.5 blocks the direct line, so the route goes through
        // the graph. The chain's own hop 27.0 -> 28.0 crosses the wall too and
        // is reported as a land leg; what this test pins is that a route comes
        // back and that a shore buffer is accepted.
        let wall = vec![vec![(27.5, 36.2), (27.5, 37.5)]];
        let (dir, path) = chain_graph_path(wall);
        let g = open(path.to_string_lossy().into_owned()).unwrap();
        let r = g.route(36.0, 26.0, 37.0, 28.0, 0.5).unwrap();
        assert!(r.coordinates.len() >= 2);
        assert!(r.distance_nm > 0.0);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn route_rejects_bad_arguments() {
        let (dir, path) = chain_graph_path(vec![]);
        let g = open(path.to_string_lossy().into_owned()).unwrap();
        assert!(matches!(
            g.route(36.0, 26.0, 37.0, 28.0, 6.0),
            Err(AswError::InvalidArgument { .. })
        ));
        assert!(matches!(
            g.route(f64::NAN, 26.0, 37.0, 28.0, 0.0),
            Err(AswError::InvalidArgument { .. })
        ));
        assert!(matches!(
            g.route(36.0, 26.0, 37.0, 28.0, -0.1),
            Err(AswError::InvalidArgument { .. })
        ));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn route_with_no_nodes_and_a_wall_is_no_route() {
        let empty = {
            let mut b = GraphBuilder::default();
            b.coastline_runs = vec![vec![(27.5, 36.2), (27.5, 37.5)]];
            b.build()
        };
        let dir = temp_dir("noroute");
        let path = dir.join("noroute.graph");
        empty.save(&path).unwrap();
        let g = open(path.to_string_lossy().into_owned()).unwrap();
        assert_eq!(
            g.route(36.0, 26.0, 37.0, 28.0, 0.0).unwrap_err(),
            AswError::NoRoute
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn route_recovers_after_a_panic() {
        let (dir, path) = chain_graph_path(vec![]);
        let g = open(path.to_string_lossy().into_owned()).unwrap();
        PANIC_NEXT.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(matches!(
            g.route(36.0, 26.0, 37.0, 28.0, 0.0),
            Err(AswError::Internal { .. })
        ));
        assert!(
            g.route(36.0, 26.0, 37.0, 28.0, 0.0).is_ok(),
            "mutex must not stay poisoned"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn concurrent_routes_serialise_on_one_buffer_set() {
        let (dir, path) = chain_graph_path(vec![]);
        let g = open(path.to_string_lossy().into_owned()).unwrap();
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let g = Arc::clone(&g);
                std::thread::spawn(move || {
                    g.route(36.0, 26.0, 37.0, 28.0, 0.0).unwrap().distance_nm
                })
            })
            .collect();
        let d: Vec<f64> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert!(d.iter().all(|&x| (x - d[0]).abs() < 1e-9));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
