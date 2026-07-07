//! Per-node distance-to-shore computation (build time).

use asw_core::geo_index::CoastlineIndex;
use asw_core::graph::{quantize_shore_dist, SHORE_DIST_MAX_NM};
use asw_core::h3::cell_center;
use h3o::CellIndex;
use rayon::prelude::*;

/// Compute quantized straight-line distance to the nearest coastline for each
/// cell. Output order matches input order.
pub fn compute_shore_distances(cells: &[(CellIndex, u32)], coastline: &CoastlineIndex) -> Vec<u8> {
    cells
        .par_iter()
        .map(|(cell, _)| {
            let (lat, lon) = cell_center(*cell);
            quantize_shore_dist(coastline.min_distance_nm(lon, lat, SHORE_DIST_MAX_NM))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use asw_core::geo_index::CoastlineSegment;
    use geo::LineString;

    #[test]
    fn near_and_far_cells() {
        // Vertical coastline at lon 28.0
        let line = LineString::from(vec![(28.0, 36.0), (28.0, 37.0)]);
        let coastline = CoastlineIndex::new(vec![CoastlineSegment::new(line)]);

        let near = asw_core::h3::lat_lng_to_cell(36.5, 28.05, h3o::Resolution::Nine).unwrap();
        let far = asw_core::h3::lat_lng_to_cell(36.5, 29.5, h3o::Resolution::Nine).unwrap();

        let result = compute_shore_distances(&[(near, 0), (far, 1)], &coastline);

        // Expected value computed from the actual cell center (cell centers
        // are offset from the query coords by up to ~100 m).
        let (lat, lon) = cell_center(near);
        let expected = quantize_shore_dist((lon - 28.0) * 60.0 * lat.to_radians().cos());
        assert_eq!(result[0], expected);
        assert_eq!(result[1], 255, "cell ~72 nm from shore must saturate");
    }
}
