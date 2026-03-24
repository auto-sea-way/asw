pub mod astar_pool;
pub mod geo_index;
pub mod graph;
pub mod h3;
pub mod passages;
pub mod routing;
pub mod varint;

/// H3 resolution for the coarsest ocean tier
pub const H3_RES_BASE: u8 = 3;
/// H3 resolution for the finest shoreline tier
pub const H3_RES_LEAF: u8 = 10;

/// Adaptive refinement cascade: (resolution, distance_threshold_deg).
/// If a cell at `resolution` is closer than `threshold` to the coastline,
/// it gets refined to `resolution + 1`. The leaf resolution (10) has no entry.
///
/// ```text
/// res-3: ocean        (edge ~32nm)  — threshold 0.30° (~18nm)   → refine to res-4
/// res-4: deep-mid     (edge ~12nm)  — threshold 0.15° (~9.2nm)  → refine to res-5
/// res-5: mid          (edge ~4.5nm) — threshold 0.05° (~3.0nm)  → refine to res-6
/// res-6: near-mid     (edge ~1.7nm) — threshold 0.025° (~1.5nm) → refine to res-7
/// res-7: coastal      (edge ~0.65nm)— threshold 0.012° (~0.70nm)→ refine to res-8
/// res-8: near-coast   (edge ~461m)  — threshold 0.005° (~550m) → refine to res-9
/// res-9: near-shore   (edge ~174m)  — threshold 0.002° (~220m) → refine to res-10
/// res-10: shoreline   (edge ~66m)   — leaf level, no refinement
/// ```
pub const CASCADE: &[(u8, f64)] = &[
    (3, 0.30),
    (4, 0.15),
    (5, 0.05),
    (6, 0.025),
    (7, 0.012),
    (8, 0.005),
    (9, 0.002),
];

/// Max vertices per coastline segment for R-tree indexing
pub const COASTLINE_SUBDIVIDE_MAX: usize = 256;
