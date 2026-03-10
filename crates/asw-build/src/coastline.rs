use geo::{Coord, LineString, Polygon};
use rayon::prelude::*;
use asw_core::geo_index::CoastlineSegment;
use asw_core::COASTLINE_SUBDIVIDE_MAX;
use tracing::info;

/// Extract coastline segments from land polygons and subdivide for R-tree indexing.
/// Returns (CoastlineSegments for R-tree, serializable coords for graph file).
pub fn extract_coastline(
    polygons: &[Polygon<f64>],
) -> (Vec<CoastlineSegment>, Vec<Vec<(f32, f32)>>) {
    info!(
        "Extracting coastline from {} polygons...",
        polygons.len()
    );

    let results: Vec<Vec<LineString<f64>>> = polygons
        .par_iter()
        .map(|poly| {
            let mut rings = Vec::new();
            // Exterior ring
            let ext = poly.exterior().clone();
            rings.extend(subdivide_ring(&ext));
            // Interior rings (holes — also coastline boundaries)
            for hole in poly.interiors() {
                rings.extend(subdivide_ring(hole));
            }
            rings
        })
        .collect();

    let all_segments: Vec<LineString<f64>> = results.into_iter().flatten().collect();
    info!("{} coastline segments after subdivision", all_segments.len());

    let coastline_segments: Vec<CoastlineSegment> = all_segments
        .iter()
        .map(|ls| CoastlineSegment::new(ls.clone()))
        .collect();

    let coastline_coords: Vec<Vec<(f32, f32)>> = all_segments
        .iter()
        .map(|ls| {
            ls.coords()
                .map(|c| (c.x as f32, c.y as f32))
                .collect()
        })
        .collect();

    (coastline_segments, coastline_coords)
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
