use asw_core::COASTLINE_SUBDIVIDE_MAX;
use geo::{Coord, LineString, Polygon};
use rayon::prelude::*;
use tracing::info;

/// Extract coastline runs (lon, lat) from land polygons, subdivided to at
/// most COASTLINE_SUBDIVIDE_MAX vertices so the grid index stays selective.
pub fn extract_coastline(polygons: &[Polygon<f64>]) -> Vec<Vec<(f64, f64)>> {
    info!("Extracting coastline from {} polygons...", polygons.len());
    let runs: Vec<Vec<(f64, f64)>> = polygons
        .par_iter()
        .flat_map_iter(|poly| {
            let mut rings = subdivide_ring(poly.exterior());
            for hole in poly.interiors() {
                rings.extend(subdivide_ring(hole));
            }
            rings
                .into_iter()
                .map(|ls| ls.coords().map(|c| (c.x, c.y)).collect::<Vec<_>>())
        })
        .collect();
    info!("{} coastline runs after subdivision", runs.len());
    runs
}

/// Subdivide a ring into segments of at most COASTLINE_SUBDIVIDE_MAX vertices.
fn subdivide_ring(ring: &LineString<f64>) -> Vec<LineString<f64>> {
    let coords: Vec<Coord<f64>> = ring.coords().cloned().collect();
    if coords.len() <= COASTLINE_SUBDIVIDE_MAX {
        return vec![ring.clone()];
    }

    let mut segments = Vec::new();
    let mut start = 0;
    while start < coords.len() - 1 {
        let end = (start + COASTLINE_SUBDIVIDE_MAX).min(coords.len());
        let segment_coords = coords[start..end].to_vec();
        if segment_coords.len() >= 2 {
            segments.push(LineString::new(segment_coords));
        }
        // Overlap by 1 vertex to maintain continuity
        start = end - 1;
    }
    segments
}
