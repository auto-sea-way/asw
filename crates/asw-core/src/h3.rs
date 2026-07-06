use h3o::{CellIndex, LatLng, Resolution};

/// Get the center coordinates of an H3 cell as (lat, lon) in degrees.
pub fn cell_center(cell: CellIndex) -> (f64, f64) {
    let ll = LatLng::from(cell);
    (ll.lat(), ll.lng())
}

/// Get all neighbor cells at distance 1 (the 6 immediate neighbors).
pub fn neighbors(cell: CellIndex) -> Vec<CellIndex> {
    cell.grid_ring::<Vec<_>>(1)
}

/// Get the parent of a cell at the given resolution.
pub fn parent(cell: CellIndex, res: Resolution) -> Option<CellIndex> {
    cell.parent(res)
}

/// Get all children of a cell at the given resolution.
pub fn children(cell: CellIndex, res: Resolution) -> Vec<CellIndex> {
    cell.children(res).collect()
}

/// Convert (lat, lon) degrees to the nearest H3 cell at given resolution.
pub fn lat_lng_to_cell(lat: f64, lng: f64, res: Resolution) -> Option<CellIndex> {
    let ll = LatLng::new(lat, lng).ok()?;
    Some(ll.to_cell(res))
}

/// Get the resolution of a cell.
pub fn resolution(cell: CellIndex) -> u8 {
    cell.resolution() as u8
}

/// Get the boundary vertices of an H3 cell as Vec<(lat, lon)> in degrees.
pub fn cell_boundary(cell: CellIndex) -> Vec<(f64, f64)> {
    let boundary = cell.boundary();
    boundary.iter().map(|ll| (ll.lat(), ll.lng())).collect()
}

/// Convert an H3 cell boundary to a geo::Polygon (lon/lat coordinates).
///
/// Cells straddling the antimeridian have h3o boundary vertices that jump between
/// +180 and -180 (e.g. 179.98, -179.97). Built naively, that ring sweeps across the
/// whole map at that latitude and gets misclassified by land/water tests. Longitudes
/// are "unwrapped" here into a continuous range (adding/subtracting 360 as needed) so
/// the ring stays compact; its coordinates may then fall slightly outside [-180, 180]
/// for a transmeridian cell. `LandIndex::intersects_polygon`/`contains_polygon` know
/// how to query such polygons correctly (see `transmeridian_variants` in geo_index.rs).
pub fn cell_polygon(cell: CellIndex) -> geo::Polygon<f64> {
    let boundary = cell.boundary();
    let mut coords: Vec<geo::Coord<f64>> = Vec::with_capacity(boundary.len() + 1);
    let mut prev_raw_lon: Option<f64> = None;
    let mut offset = 0.0_f64;
    for ll in boundary.iter() {
        let raw_lon = ll.lng();
        if let Some(prev) = prev_raw_lon {
            let delta = raw_lon - prev;
            if delta > 180.0 {
                offset -= 360.0;
            } else if delta < -180.0 {
                offset += 360.0;
            }
        }
        prev_raw_lon = Some(raw_lon);
        coords.push(geo::Coord {
            x: raw_lon + offset,
            y: ll.lat(),
        });
    }
    // Close the ring
    if let Some(&first) = coords.first() {
        coords.push(first);
    }
    geo::Polygon::new(geo::LineString::new(coords), vec![])
}

/// Haversine distance in nautical miles between two (lat, lon) points.
pub fn haversine_nm(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    use std::f64::consts::PI;
    let r = 3440.065; // Earth radius in nm
    let dlat = (lat2 - lat1) * PI / 180.0;
    let dlon = (lon2 - lon1) * PI / 180.0;
    let a = (dlat / 2.0).sin().powi(2)
        + (lat1 * PI / 180.0).cos() * (lat2 * PI / 180.0).cos() * (dlon / 2.0).sin().powi(2);
    let c = 2.0 * a.sqrt().asin();
    r * c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn haversine_zero_distance() {
        let d = haversine_nm(51.5074, -0.1278, 51.5074, -0.1278);
        assert!((d - 0.0).abs() < 1e-10);
    }

    #[test]
    fn haversine_london_paris() {
        let d = haversine_nm(51.5074, -0.1278, 48.8566, 2.3522);
        assert!(
            (d - 186.0).abs() < 3.0,
            "London-Paris was {d} nm, expected ~186"
        );
    }

    #[test]
    fn haversine_antipodal() {
        let d = haversine_nm(90.0, 0.0, -90.0, 0.0);
        assert!(
            (d - 10808.0).abs() < 55.0,
            "Antipodal was {d} nm, expected ~10808"
        );
    }

    #[test]
    fn haversine_symmetry() {
        let d1 = haversine_nm(51.5074, -0.1278, 48.8566, 2.3522);
        let d2 = haversine_nm(48.8566, 2.3522, 51.5074, -0.1278);
        assert!((d1 - d2).abs() < 1e-10);
    }

    /// A res-5 cell seeded right at the antimeridian (lat 65, lon 179.99) must produce
    /// a compact polygon (small lon extent), not a degenerate ring spanning ~360 degrees.
    /// Pre-fix, cell_polygon builds coords straight from h3o's wrapped boundary lng()
    /// values, so a cell straddling the seam gets vertices on both +179.x and -179.x,
    /// and the raw ring spans nearly the full longitude range.
    #[test]
    fn cell_polygon_transmeridian_cell_has_small_lon_extent() {
        let ll = LatLng::new(65.0, 179.99).expect("valid lat/lng");
        let cell = ll.to_cell(Resolution::Five);

        // Sanity: confirm this cell actually straddles the antimeridian, i.e. the raw
        // h3o boundary has a jump > 180 degrees between two consecutive vertices.
        let boundary: Vec<_> = cell.boundary().iter().copied().collect();
        let n = boundary.len();
        let straddles = (0..n).any(|i| {
            let a = boundary[i].lng();
            let b = boundary[(i + 1) % n].lng();
            (a - b).abs() > 180.0
        });
        assert!(
            straddles,
            "test cell does not actually straddle the antimeridian; pick a different seed point"
        );

        let poly = cell_polygon(cell);
        let lons: Vec<f64> = poly.exterior().coords().map(|c| c.x).collect();
        let min_lon = lons.iter().cloned().fold(f64::MAX, f64::min);
        let max_lon = lons.iter().cloned().fold(f64::MIN, f64::max);
        assert!(
            max_lon - min_lon < 5.0,
            "transmeridian cell polygon should have small lon extent, got {} (min={}, max={})",
            max_lon - min_lon,
            min_lon,
            max_lon
        );
    }

    #[test]
    fn cell_polygon_normal_cell_unaffected() {
        // A cell far from the antimeridian should produce an ordinary small polygon,
        // same as before.
        let ll = LatLng::new(51.5, -0.1).expect("valid lat/lng");
        let cell = ll.to_cell(Resolution::Five);
        let poly = cell_polygon(cell);
        let lons: Vec<f64> = poly.exterior().coords().map(|c| c.x).collect();
        let min_lon = lons.iter().cloned().fold(f64::MAX, f64::min);
        let max_lon = lons.iter().cloned().fold(f64::MIN, f64::max);
        assert!(max_lon - min_lon < 1.0);
    }
}
