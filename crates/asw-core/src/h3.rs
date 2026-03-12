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
pub fn cell_polygon(cell: CellIndex) -> geo::Polygon<f64> {
    let boundary = cell.boundary();
    let mut coords: Vec<geo::Coord<f64>> = boundary
        .iter()
        .map(|ll| geo::Coord {
            x: ll.lng(),
            y: ll.lat(),
        })
        .collect();
    // Close the ring
    if let Some(&first) = coords.first() {
        coords.push(first);
    }
    geo::Polygon::new(geo::LineString::new(coords), vec![])
}

/// Haversine distance in kilometers between two (lat, lon) points.
pub fn haversine_km(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    use std::f64::consts::PI;
    let r = 6371.0; // Earth radius in km
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
        let d = haversine_km(51.5074, -0.1278, 51.5074, -0.1278);
        assert!((d - 0.0).abs() < 1e-10);
    }

    #[test]
    fn haversine_london_paris() {
        let d = haversine_km(51.5074, -0.1278, 48.8566, 2.3522);
        assert!(
            (d - 344.0).abs() < 5.0,
            "London-Paris was {d} km, expected ~344"
        );
    }

    #[test]
    fn haversine_antipodal() {
        let d = haversine_km(90.0, 0.0, -90.0, 0.0);
        assert!(
            (d - 20015.0).abs() < 100.0,
            "Antipodal was {d} km, expected ~20015"
        );
    }

    #[test]
    fn haversine_symmetry() {
        let d1 = haversine_km(51.5074, -0.1278, 48.8566, 2.3522);
        let d2 = haversine_km(48.8566, 2.3522, 51.5074, -0.1278);
        assert!((d1 - d2).abs() < 1e-10);
    }
}
