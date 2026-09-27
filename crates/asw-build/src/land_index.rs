//! Land polygon R-tree used only at build time.

use geo::algorithm::bool_ops::BooleanOps;
use geo::{BoundingRect, Contains, Coord, Intersects, LineString, MultiPolygon, Point, Polygon};
use rayon::prelude::*;
use rstar::{Envelope, RTree, RTreeObject, AABB};
use tracing::info;

/// Rect→AABB-corners adapter over geo's `BoundingRect`.
fn corners<G: BoundingRect<f64, Output = Option<geo::Rect<f64>>>>(g: &G) -> ([f64; 2], [f64; 2]) {
    let r = g.bounding_rect().expect("non-empty geometry");
    ([r.min().x, r.min().y], [r.max().x, r.max().y])
}

/// A land polygon stored in the R-tree with its bounding envelope.
#[derive(Clone, Debug)]
pub struct LandPolygon {
    pub polygon: Polygon<f64>,
    envelope: AABB<[f64; 2]>,
}

impl LandPolygon {
    pub fn new(polygon: Polygon<f64>) -> Self {
        let (min, max) = corners(&polygon);
        let envelope = AABB::from_corners(min, max);
        Self { polygon, envelope }
    }
}

impl RTreeObject for LandPolygon {
    type Envelope = AABB<[f64; 2]>;
    fn envelope(&self) -> Self::Envelope {
        self.envelope
    }
}

/// Spatial index for land polygons. Points NOT inside any land polygon are water.
pub struct LandIndex {
    tree: RTree<LandPolygon>,
}

impl LandIndex {
    pub fn new(polygons: Vec<LandPolygon>) -> Self {
        let tree = RTree::bulk_load(polygons);
        Self { tree }
    }

    /// Check if a point (lon, lat) is in water (i.e. NOT inside any land polygon).
    pub fn is_water(&self, lon: f64, lat: f64) -> bool {
        let point = Point::new(lon, lat);
        let envelope = AABB::from_corners([lon, lat], [lon, lat]);
        for lp in self.tree.locate_in_envelope_intersecting(envelope) {
            if lp.polygon.contains(&point) {
                return false;
            }
        }
        true
    }

    /// Check if any land polygon intersects the given polygon.
    ///
    /// Antimeridian-aware: a polygon produced by `cell_polygon` for a transmeridian
    /// H3 cell may carry unwrapped longitudes outside [-180, 180] (see h3.rs). Stored
    /// land polygons always live within [-180, 180], split at the seam, so such a
    /// query polygon is tested once as-is and once shifted back into range — this
    /// catches land on either side of the antimeridian without reintroducing the
    /// degenerate world-spanning ring the unwrapping was meant to avoid.
    pub fn intersects_polygon(&self, poly: &Polygon<f64>) -> bool {
        // Fast path: non-transmeridian polygon, no allocation or cloning
        if !has_transmeridian_coords(poly) {
            return self.intersects_polygon_single(poly);
        }
        // Transmeridian case: build and test variants
        transmeridian_variants(poly)
            .iter()
            .any(|variant| self.intersects_polygon_single(variant))
    }

    fn intersects_polygon_single(&self, poly: &Polygon<f64>) -> bool {
        let (min, max) = corners(poly);
        let envelope = AABB::from_corners(min, max);
        for lp in self.tree.locate_in_envelope_intersecting(envelope) {
            if lp.polygon.intersects(poly) {
                return true;
            }
        }
        false
    }

    /// Check if the given polygon is entirely contained within any single land polygon.
    /// Antimeridian-aware in the same way as `intersects_polygon`.
    pub fn contains_polygon(&self, poly: &Polygon<f64>) -> bool {
        // Fast path: non-transmeridian polygon, no allocation or cloning
        if !has_transmeridian_coords(poly) {
            return self.contains_polygon_single(poly);
        }
        // Transmeridian case: build and test variants
        transmeridian_variants(poly)
            .iter()
            .any(|variant| self.contains_polygon_single(variant))
    }

    fn contains_polygon_single(&self, poly: &Polygon<f64>) -> bool {
        let (min, max) = corners(poly);
        let envelope = AABB::from_corners(min, max);
        for lp in self.tree.locate_in_envelope_intersecting(envelope) {
            if lp.polygon.contains(poly) {
                return true;
            }
        }
        false
    }

    pub fn polygon_count(&self) -> usize {
        self.tree.size()
    }

    /// Extract all land polygons from the R-tree.
    /// Used to get post-subtraction polygons for coastline extraction.
    pub fn polygons(&self) -> Vec<Polygon<f64>> {
        self.tree.iter().map(|lp| lp.polygon.clone()).collect()
    }

    /// Subtract water polygons from land, creating holes where canals exist.
    /// Uses a water R-tree to find only the relevant water polygons per land polygon,
    /// then applies BooleanOps difference in parallel via rayon.
    pub fn subtract_water(&mut self, water_polygons: &[Polygon<f64>]) {
        if water_polygons.is_empty() {
            return;
        }

        // Build R-tree of water polygons for fast spatial lookup
        let water_entries: Vec<LandPolygon> = water_polygons
            .iter()
            .cloned()
            .map(LandPolygon::new)
            .collect();
        let water_tree = RTree::bulk_load(water_entries);

        // Compute water bounding box for quick global filtering
        let water_envelope = water_polygons.iter().map(corners).fold(
            ([f64::MAX, f64::MAX], [f64::MIN, f64::MIN]),
            |(acc_min, acc_max), (min, max)| {
                (
                    [acc_min[0].min(min[0]), acc_min[1].min(min[1])],
                    [acc_max[0].max(max[0]), acc_max[1].max(max[1])],
                )
            },
        );
        let water_envelope = AABB::from_corners(water_envelope.0, water_envelope.1);

        let candidates: Vec<LandPolygon> = self.tree.iter().cloned().collect();
        let total = candidates.len();
        let intersecting = candidates
            .iter()
            .filter(|lp| lp.envelope.intersects(&water_envelope))
            .count();
        info!(
            "subtract_water: {} land polygons, {} intersect water bbox, {} water polygons",
            total,
            intersecting,
            water_polygons.len()
        );

        // Parallel BooleanOps — each land polygon only subtracts nearby water polygons
        let all_polys: Vec<LandPolygon> = candidates
            .into_par_iter()
            .flat_map(|lp| {
                if !lp.envelope.intersects(&water_envelope) {
                    return vec![lp];
                }
                // Find water polygons that intersect this land polygon's bbox
                let nearby_water: Vec<&Polygon<f64>> = water_tree
                    .locate_in_envelope_intersecting(lp.envelope)
                    .map(|wp| &wp.polygon)
                    .collect();
                if nearby_water.is_empty() {
                    return vec![lp];
                }
                // Subtract only the nearby water polygons
                let water_multi = MultiPolygon::new(nearby_water.into_iter().cloned().collect());
                let diff = lp.polygon.difference(&water_multi);
                diff.into_iter().map(LandPolygon::new).collect::<Vec<_>>()
            })
            .collect();

        info!(
            "subtract_water: {} polygons after subtraction",
            all_polys.len()
        );
        self.tree = RTree::bulk_load(all_polys);
    }
}

/// Check if any coordinate in the polygon falls outside [-180, 180].
/// Used as a fast-path check to avoid allocation for the common non-transmeridian case.
fn has_transmeridian_coords(poly: &Polygon<f64>) -> bool {
    poly.exterior()
        .coords()
        .any(|c| c.x > 180.0 || c.x < -180.0)
}

/// Produce the polygon variants needed to correctly test a possibly-unwrapped
/// transmeridian polygon (see `h3::cell_polygon`) against a `LandIndex`, whose stored
/// polygons always live within [-180, 180].
///
/// Only called when `has_transmeridian_coords` returns true. Returns a vector with
/// the original polygon and optionally shifted variants so that overflowing portions
/// land back in valid coordinate space and can match land polygons on the far side of
/// the seam, while in-range portions still match via the original copy.
fn transmeridian_variants(poly: &Polygon<f64>) -> Vec<Polygon<f64>> {
    let has_over = poly.exterior().coords().any(|c| c.x > 180.0);
    let has_under = poly.exterior().coords().any(|c| c.x < -180.0);

    let mut variants = vec![poly.clone()];
    if has_over {
        variants.push(shift_polygon(poly, -360.0));
    }
    if has_under {
        variants.push(shift_polygon(poly, 360.0));
    }
    variants
}

fn shift_polygon(poly: &Polygon<f64>, dx: f64) -> Polygon<f64> {
    let shifted: Vec<Coord<f64>> = poly
        .exterior()
        .coords()
        .map(|c| Coord {
            x: c.x + dx,
            y: c.y,
        })
        .collect();
    Polygon::new(LineString::new(shifted), vec![])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `cell_polygon` (h3.rs) unwraps transmeridian cells into a compact ring whose
    /// longitudes may fall slightly outside [-180, 180]. LandIndex queries must still
    /// find land on either physical side of the seam for such a polygon.
    #[test]
    fn land_index_intersects_polygon_handles_unwrapped_transmeridian_ring() {
        fn square(x0: f64, x1: f64, y0: f64, y1: f64) -> Polygon<f64> {
            Polygon::new(
                LineString::new(vec![
                    Coord { x: x0, y: y0 },
                    Coord { x: x1, y: y0 },
                    Coord { x: x1, y: y1 },
                    Coord { x: x0, y: y1 },
                    Coord { x: x0, y: y0 },
                ]),
                vec![],
            )
        }

        // Simulate an unwrapped transmeridian cell polygon straddling the seam: a raw
        // vertex at -179.5 becomes 180.5 once unwrapped, giving a continuous ring
        // spanning lon 179.5..180.5.
        let poly = square(179.5, 180.5, 0.0, 1.0);

        // Land just west of the seam (raw lon around -179.8) — only reachable via the
        // shifted (+360) variant.
        let index_west = LandIndex::new(vec![LandPolygon::new(square(-179.9, -179.7, 0.4, 0.6))]);
        assert!(
            index_west.intersects_polygon(&poly),
            "transmeridian polygon must detect land just west of the seam"
        );

        // Land just east of the seam (raw lon around 179.6) — reachable directly.
        let index_east = LandIndex::new(vec![LandPolygon::new(square(179.55, 179.65, 0.4, 0.6))]);
        assert!(
            index_east.intersects_polygon(&poly),
            "transmeridian polygon must detect land just east of the seam"
        );

        // Unrelated land far away (near the prime meridian) must not match.
        let index_far = LandIndex::new(vec![LandPolygon::new(square(0.0, 0.1, 0.4, 0.6))]);
        assert!(!index_far.intersects_polygon(&poly));
    }
}
