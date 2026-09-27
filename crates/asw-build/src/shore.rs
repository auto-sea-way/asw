//! Per-node distance-to-shore computation (build time).

use asw_core::coast::CoastlineIndex;
use asw_core::graph::{quantize_shore_dist, SHORE_DIST_MAX_NM};
use asw_core::h3::cell_center;
use h3o::CellIndex;
use rayon::prelude::*;

/// Compute quantized straight-line distance to the nearest coastline for each
/// cell. Output order matches input order.
pub fn compute_shore_distances(
    cells: &[(CellIndex, u32)],
    coastline: &CoastlineIndex<'_>,
) -> Vec<u8> {
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

    #[test]
    fn near_and_far_cells() {
        // Vertical coastline at lon 28.0
        let sections =
            asw_core::coast::CoastlineSections::from_runs(&[vec![(28.0, 36.0), (28.0, 37.0)]]);
        let coastline = sections.index();

        let near = h3o::LatLng::new(36.5, 28.05)
            .unwrap()
            .to_cell(h3o::Resolution::Nine);
        let far = h3o::LatLng::new(36.5, 29.5)
            .unwrap()
            .to_cell(h3o::Resolution::Nine);

        let result = compute_shore_distances(&[(near, 0), (far, 1)], &coastline);

        // Expected value computed from the actual cell center (cell centers
        // are offset from the query coords by up to ~100 m).
        let (lat, lon) = cell_center(near);
        let expected = quantize_shore_dist((lon - 28.0) * 60.0 * lat.to_radians().cos());
        assert_eq!(result[0], expected);
        assert_eq!(result[1], 255, "cell ~72 nm from shore must saturate");
    }
}
