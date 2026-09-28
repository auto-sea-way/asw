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

/// An opened graph file. Reference counted across the FFI; safe to share
/// between threads. Routes serialise on the single A* buffer set.
#[derive(uniffi::Object)]
pub struct Graph {
    inner: RoutingGraph,
    // Read by `route` (Task 3); the allow goes with it.
    #[allow(dead_code)]
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use asw_core::graph::GraphBuilder;
    use std::path::PathBuf;

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
        let dir = std::env::temp_dir().join(format!(
            "asw-mobile-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
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
}
