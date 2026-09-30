//! Mobile bindings for the auto-sea-way routing graph. Four calls over a
//! memory-mapped v5 file: `open_graph`, `version`, `is_water`, `route`. Generated
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
    // `detail`, not `message`: Kotlin's Throwable already has `message`.
    BadFormat { detail: String },
    InvalidArgument { detail: String },
    NoRoute,
    Internal { detail: String },
}

impl std::fmt::Display for AswError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AswError::NotFound => write!(f, "graph file not found"),
            AswError::BadFormat { detail } => write!(f, "not a usable graph file: {detail}"),
            AswError::InvalidArgument { detail } => write!(f, "invalid argument: {detail}"),
            AswError::NoRoute => write!(f, "no route between the given points"),
            AswError::Internal { detail } => write!(f, "internal error: {detail}"),
        }
    }
}

impl std::error::Error for AswError {}

/// Answer of `Graph::is_water`. `Unknown` when the call could not be
/// evaluated: invalid coordinates or a panic inside the graph code. A file
/// changed in place under the mapping is not covered (see the crate README).
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

// Test-only one-shot switches that make the next `is_water` or `route` on
// this thread panic, proving the panic never crosses the boundary.
// Thread-local, so parallel tests cannot consume each other's panic.
#[cfg(test)]
thread_local! {
    static PANIC_NEXT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static PANIC_AFTER_SEARCH: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
fn maybe_panic() {
    if PANIC_NEXT.with(|c| c.replace(false)) {
        panic!("forced panic for the boundary test");
    }
}

#[cfg(not(test))]
fn maybe_panic() {}

/// Panics after the search has stamped the A* buffers: the window a
/// mid-route panic leaves behind.
#[cfg(test)]
fn maybe_panic_after_search() {
    if PANIC_AFTER_SEARCH.with(|c| c.replace(false)) {
        panic!("forced panic after the search");
    }
}

#[cfg(not(test))]
fn maybe_panic_after_search() {}

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

/// Finite and within the WGS84 ranges (lat -90..=90, lon -180..=180).
fn valid_point(lat: f64, lon: f64) -> bool {
    lat.is_finite()
        && lon.is_finite()
        && (-90.0..=90.0).contains(&lat)
        && (-180.0..=180.0).contains(&lon)
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

/// Memory-map a v5 graph file and validate its header. Milliseconds; no
/// per-node work. Named `open_graph` because `open` needs backticks in both
/// Swift and Kotlin.
#[uniffi::export]
pub fn open_graph(path: String) -> Result<Arc<Graph>, AswError> {
    let p = Path::new(&path);
    let meta = std::fs::metadata(p).map_err(|_| AswError::NotFound)?;
    if !meta.is_file() {
        return Err(AswError::BadFormat {
            detail: "not a regular file".into(),
        });
    }
    if meta.len() < 4 {
        return Err(AswError::BadFormat {
            detail: "file is empty or truncated".into(),
        });
    }
    let opened = catch_unwind(AssertUnwindSafe(|| RoutingGraph::open(p, false))).map_err(|e| {
        AswError::Internal {
            detail: panic_message(e),
        }
    })?;
    // An I/O failure (unreadable, vanished between metadata and open) is
    // NotFound; anything the reader rejects is BadFormat with its message.
    let inner = opened.map_err(|e| {
        if e.downcast_ref::<std::io::Error>().is_some() {
            AswError::NotFound
        } else {
            AswError::BadFormat {
                detail: format!("{e:#}"),
            }
        }
    })?;
    Ok(Arc::new(Graph {
        inner,
        buffers: Mutex::new(None),
    }))
}

/// Deletes the temporary file unless the install succeeded (`keep`).
struct TempFile {
    path: std::path::PathBuf,
    keep: bool,
}

impl Drop for TempFile {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Install a zstd-compressed graph file (the release's `asw.graph.zst`) at
/// `destination`: decompress into a temporary file next to it, check that
/// it is a usable v5 graph, then rename it over `destination` atomically.
/// On any failure the destination is untouched and nothing is left behind.
/// A `Graph` already open on the old file keeps working until released.
#[uniffi::export]
pub fn install_graph(source: String, destination: String) -> Result<(), AswError> {
    catch_unwind(AssertUnwindSafe(|| {
        install_graph_inner(&source, &destination)
    }))
    .unwrap_or_else(|e| {
        Err(AswError::Internal {
            detail: panic_message(e),
        })
    })
}

fn install_graph_inner(source: &str, destination: &str) -> Result<(), AswError> {
    use std::io::{Read, Write};
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let internal = |e: std::io::Error| AswError::Internal {
        detail: e.to_string(),
    };
    let bad = |e: std::io::Error| AswError::BadFormat {
        detail: format!("not a zstd-compressed graph: {e}"),
    };

    let input = std::fs::File::open(source).map_err(|_| AswError::NotFound)?;
    let dest = Path::new(destination);
    let dir = dest
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = dest
        .file_name()
        .ok_or_else(|| AswError::InvalidArgument {
            detail: "destination has no file name".into(),
        })?
        .to_string_lossy();
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut tmp = TempFile {
        path: dir.join(format!(".{name}.install-{}-{n}", std::process::id())),
        keep: false,
    };

    let mut out = std::fs::File::create(&tmp.path).map_err(internal)?;
    let mut decoder = zstd::stream::read::Decoder::new(input).map_err(bad)?;
    // Read and write separately so bad data (BadFormat) and a failing disk
    // (Internal) are told apart. The zstd frame checksum is verified by the
    // decoder at the end of the stream.
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let k = decoder.read(&mut buf).map_err(bad)?;
        if k == 0 {
            break;
        }
        out.write_all(&buf[..k]).map_err(internal)?;
    }
    out.sync_all().map_err(internal)?;
    drop(out);

    // Header check: the decompressed file must be a v5 graph this build reads.
    RoutingGraph::open(&tmp.path, false).map_err(|e| AswError::BadFormat {
        detail: format!("{e:#}"),
    })?;

    std::fs::rename(&tmp.path, dest).map_err(internal)?;
    tmp.keep = true;
    Ok(())
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
        if !valid_point(lat, lon) {
            return Water::Unknown;
        }
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
    /// nautical miles (0 to 5). `arctic` opens the seasonal Arctic passages;
    /// the ice cap north of 80N is always closed. Blocking; routes serialise
    /// on one buffer set.
    #[uniffi::method(default(arctic = false))]
    pub fn route(
        &self,
        from_lat: f64,
        from_lon: f64,
        to_lat: f64,
        to_lon: f64,
        shore_buffer_nm: f64,
        arctic: bool,
    ) -> Result<Route, AswError> {
        for (name, lat, lon) in [("from", from_lat, from_lon), ("to", to_lat, to_lon)] {
            if !valid_point(lat, lon) {
                return Err(AswError::InvalidArgument {
                    detail: format!("{name} is not a finite lat/lon within -90..90 and -180..180"),
                });
            }
        }
        if !shore_buffer_nm.is_finite() || !(0.0..=5.0).contains(&shore_buffer_nm) {
            return Err(AswError::InvalidArgument {
                detail: "shore_buffer_nm must be between 0 and 5 nautical miles".into(),
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
            // Reset before the search, not after: a panic mid-route would
            // otherwise leave the previous generation's closed flags and
            // heuristics live for the next call.
            buffers.reset();
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
                arctic,
            );
            maybe_panic_after_search();
            result
        }));
        match outcome {
            Err(payload) => Err(AswError::Internal {
                detail: panic_message(payload),
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
    /// Written to a temporary directory as a v5 file.
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
        let g = open_graph(path.to_string_lossy().into_owned()).unwrap();
        assert_eq!(g.version(), "test 2026-09-28");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn open_missing_file_is_not_found() {
        let err = open_graph("/nonexistent/asw.graph".into()).unwrap_err();
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
            match open_graph(p.to_string_lossy().into_owned()) {
                Err(AswError::BadFormat { detail }) => assert!(!detail.is_empty()),
                other => panic!("{p:?}: expected BadFormat, got {other:?}"),
            }
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn error_display_is_readable() {
        assert_eq!(AswError::NotFound.to_string(), "graph file not found");
        assert_eq!(
            AswError::BadFormat { detail: "x".into() }.to_string(),
            "not a usable graph file: x"
        );
    }

    #[test]
    fn is_water_marina_behind_mole_is_water() {
        let (dir, path) = fixture_graph_path();
        let g = open_graph(path.to_string_lossy().into_owned()).unwrap();
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
        let g = open_graph(path.to_string_lossy().into_owned()).unwrap();
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
        let g = open_graph(path.to_string_lossy().into_owned()).unwrap();
        assert_eq!(g.is_water(44.8, 20.5), Water::Land);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn is_water_panic_becomes_unknown() {
        let (dir, path) = fixture_graph_path();
        let g = open_graph(path.to_string_lossy().into_owned()).unwrap();
        PANIC_NEXT.with(|c| c.set(true));
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
        let g = open_graph(path.to_string_lossy().into_owned()).unwrap();
        let r = g.route(36.0, 26.0, 37.0, 28.0, 0.0, false).unwrap();
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
        let g = open_graph(path.to_string_lossy().into_owned()).unwrap();
        let r = g.route(36.0, 26.0, 37.0, 28.0, 0.5, false).unwrap();
        assert!(r.coordinates.len() >= 2);
        assert!(r.distance_nm > 0.0);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn route_rejects_bad_arguments() {
        let (dir, path) = chain_graph_path(vec![]);
        let g = open_graph(path.to_string_lossy().into_owned()).unwrap();
        assert!(matches!(
            g.route(36.0, 26.0, 37.0, 28.0, 6.0, false),
            Err(AswError::InvalidArgument { .. })
        ));
        assert!(matches!(
            g.route(f64::NAN, 26.0, 37.0, 28.0, 0.0, false),
            Err(AswError::InvalidArgument { .. })
        ));
        assert!(matches!(
            g.route(36.0, 26.0, 37.0, 28.0, -0.1, false),
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
        let g = open_graph(path.to_string_lossy().into_owned()).unwrap();
        assert_eq!(
            g.route(36.0, 26.0, 37.0, 28.0, 0.0, false).unwrap_err(),
            AswError::NoRoute
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn route_recovers_after_a_panic() {
        let (dir, path) = chain_graph_path(vec![]);
        let g = open_graph(path.to_string_lossy().into_owned()).unwrap();
        PANIC_NEXT.with(|c| c.set(true));
        assert!(matches!(
            g.route(36.0, 26.0, 37.0, 28.0, 0.0, false),
            Err(AswError::Internal { .. })
        ));
        assert!(
            g.route(36.0, 26.0, 37.0, 28.0, 0.0, false).is_ok(),
            "mutex must not stay poisoned"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn concurrent_routes_serialise_on_one_buffer_set() {
        let (dir, path) = chain_graph_path(vec![]);
        let g = open_graph(path.to_string_lossy().into_owned()).unwrap();
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let g = Arc::clone(&g);
                std::thread::spawn(move || {
                    g.route(36.0, 26.0, 37.0, 28.0, 0.0, false)
                        .unwrap()
                        .distance_nm
                })
            })
            .collect();
        let d: Vec<f64> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert!(d.iter().all(|&x| (x - d[0]).abs() < 1e-9));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn route_after_a_panic_past_the_search_does_not_reuse_stale_buffers() {
        // A panic after A* has stamped the buffers but before they are reset
        // must not leave closed flags and heuristics live for the next route.
        let (dir, path) = chain_graph_path(vec![vec![(27.5, 36.2), (27.5, 37.5)]]);
        let g = open_graph(path.to_string_lossy().into_owned()).unwrap();
        let first = g.route(36.0, 26.0, 37.0, 28.0, 0.0, false).unwrap();
        PANIC_AFTER_SEARCH.with(|c| c.set(true));
        assert!(matches!(
            g.route(36.0, 26.0, 37.0, 28.0, 0.0, false),
            Err(AswError::Internal { .. })
        ));
        let again = g
            .route(36.0, 26.0, 37.0, 28.0, 0.0, false)
            .expect("stale search state must not turn a valid route into NoRoute");
        assert_eq!(again, first);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn open_graph_reports_a_directory_and_an_unreadable_file_plainly() {
        let (dir, path) = fixture_graph_path();
        match open_graph(dir.to_string_lossy().into_owned()) {
            Err(AswError::BadFormat { detail }) => assert_eq!(detail, "not a regular file"),
            other => panic!("directory: expected BadFormat, got {other:?}"),
        }
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        let err = open_graph(path.to_string_lossy().into_owned()).unwrap_err();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            err,
            AswError::NotFound,
            "unreadable file is NotFound, no OS error text"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn out_of_range_coordinates_are_rejected_or_unknown() {
        let (dir, path) = chain_graph_path(vec![]);
        let g = open_graph(path.to_string_lossy().into_owned()).unwrap();
        assert!(matches!(
            g.route(200.0, 0.0, 201.0, 0.0, 0.0, false),
            Err(AswError::InvalidArgument { .. })
        ));
        assert!(matches!(
            g.route(36.0, 26.0, 37.0, 181.0, 0.0, false),
            Err(AswError::InvalidArgument { .. })
        ));
        assert_eq!(g.is_water(f64::NAN, 26.0), Water::Unknown);
        assert_eq!(g.is_water(95.0, 26.0), Water::Unknown);
        assert_eq!(g.is_water(36.0, -181.0), Water::Unknown);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A small v5 graph with the given version, written raw and as zstd.
    fn graph_files(dir: &std::path::Path, version: &str) -> (PathBuf, PathBuf) {
        let cell = h3o::LatLng::new(36.5, 28.3)
            .unwrap()
            .to_cell(h3o::Resolution::Five);
        let mut b = GraphBuilder::with_version(version);
        b.add_node(u64::from(cell), 255);
        let bytes = b.build_bytes();
        let raw = dir.join(format!("{version}.graph"));
        std::fs::write(&raw, &bytes).unwrap();
        let zst = dir.join(format!("{version}.graph.zst"));
        std::fs::write(&zst, zstd::encode_all(&bytes[..], 3).unwrap()).unwrap();
        (raw, zst)
    }

    /// Names of the files in `dir`, sorted, so a leftover temporary file shows.
    fn names(dir: &std::path::Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    fn s(p: &std::path::Path) -> String {
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn install_graph_decompresses_and_opens() {
        let dir = temp_dir("install");
        let (_, zst) = graph_files(&dir, "v1");
        let dest = dir.join("asw.graph");
        install_graph(s(&zst), s(&dest)).unwrap();
        assert_eq!(open_graph(s(&dest)).unwrap().version(), "v1");
        assert_eq!(names(&dir), vec!["asw.graph", "v1.graph", "v1.graph.zst"]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn install_graph_over_an_open_graph_keeps_the_old_one_readable() {
        let dir = temp_dir("install-over");
        let (_, zst_a) = graph_files(&dir, "a");
        let (_, zst_b) = graph_files(&dir, "b");
        let dest = dir.join("asw.graph");
        install_graph(s(&zst_a), s(&dest)).unwrap();
        let old = open_graph(s(&dest)).unwrap();
        install_graph(s(&zst_b), s(&dest)).unwrap();
        assert_eq!(old.version(), "a", "the open graph keeps its file");
        assert_eq!(old.is_water(36.5, 28.3), Water::Water);
        assert_eq!(open_graph(s(&dest)).unwrap().version(), "b");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn install_graph_failures_leave_the_destination_untouched() {
        let dir = temp_dir("install-bad");
        let (_, good) = graph_files(&dir, "good");
        let dest = dir.join("asw.graph");
        install_graph(s(&good), s(&dest)).unwrap();
        let before = std::fs::read(&dest).unwrap();

        let not_zstd = dir.join("not-zstd.zst");
        std::fs::write(&not_zstd, b"this is not a zstd stream").unwrap();
        let not_graph = dir.join("not-graph.zst");
        std::fs::write(
            &not_graph,
            zstd::encode_all(&b"hello, not a graph"[..], 3).unwrap(),
        )
        .unwrap();
        let truncated = dir.join("truncated.zst");
        let full = std::fs::read(&good).unwrap();
        std::fs::write(&truncated, &full[..full.len() / 2]).unwrap();

        for bad in [&not_zstd, &not_graph, &truncated] {
            match install_graph(s(bad), s(&dest)) {
                Err(AswError::BadFormat { detail }) => assert!(!detail.is_empty()),
                other => panic!("{bad:?}: expected BadFormat, got {other:?}"),
            }
        }
        assert_eq!(std::fs::read(&dest).unwrap(), before);
        assert_eq!(
            names(&dir),
            vec![
                "asw.graph",
                "good.graph",
                "good.graph.zst",
                "not-graph.zst",
                "not-zstd.zst",
                "truncated.zst"
            ],
            "no temporary file may be left behind"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn install_graph_missing_source_is_not_found() {
        let dir = temp_dir("install-missing");
        let err = install_graph(s(&dir.join("absent.zst")), s(&dir.join("asw.graph"))).unwrap_err();
        assert_eq!(err, AswError::NotFound);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
