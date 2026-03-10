pub mod geo_index;
pub mod graph;
pub mod h3;
pub mod passages;
pub mod routing;

/// H3 resolution for the coarsest ocean tier
pub const H3_RES_BASE: u8 = 3;
/// H3 resolution for the finest shoreline tier
pub const H3_RES_LEAF: u8 = 9;

/// Adaptive refinement cascade: (resolution, distance_threshold_deg).
/// If a cell at `resolution` is closer than `threshold` to the coastline,
/// it gets refined to `resolution + 1`. The leaf resolution (9) has no entry.
///
/// ```text
/// res-3: ocean        (edge ~59km)  — threshold 0.30° (~33km)  → refine to res-4
/// res-4: deep-mid     (edge ~22km)  — threshold 0.15° (~17km)  → refine to res-5
/// res-5: mid          (edge ~8.4km) — threshold 0.05° (~5.5km) → refine to res-6
/// res-6: near-mid     (edge ~3.2km) — threshold 0.025° (~2.8km)→ refine to res-7
/// res-7: coastal      (edge ~1.2km) — threshold 0.012° (~1.3km)→ refine to res-8
/// res-8: near-coast   (edge ~461m)  — threshold 0.005° (~550m) → refine to res-9
/// res-9: shoreline    (edge ~174m)  — leaf level, no refinement
/// ```
pub const CASCADE: &[(u8, f64)] = &[
    (3, 0.30),
    (4, 0.15),
    (5, 0.05),
    (6, 0.025),
    (7, 0.012),
    (8, 0.005),
];

/// Max vertices per coastline segment for R-tree indexing
pub const COASTLINE_SUBDIVIDE_MAX: usize = 256;
/// Max distance (km) to snap a passage waypoint to an existing node
pub const PASSAGE_SNAP_KM: f64 = 5.0;
