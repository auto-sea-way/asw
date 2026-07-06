use geo::algorithm::bool_ops::BooleanOps;
use geo::{Contains, Coord, Intersects, Line, LineString, MultiPolygon, Point, Polygon};
use rayon::prelude::*;
use rstar::{Envelope, RTree, RTreeObject, AABB};
use tracing::info;

/// A land polygon stored in the R-tree with its bounding envelope.
#[derive(Clone, Debug)]
pub struct LandPolygon {
    pub polygon: Polygon<f64>,
    envelope: AABB<[f64; 2]>,
}

impl LandPolygon {
    pub fn new(polygon: Polygon<f64>) -> Self {
        let (min, max) = bounding_rect(&polygon);
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

/// A coastline segment stored in the R-tree.
#[derive(Clone, Debug)]
pub struct CoastlineSegment {
    pub line: LineString<f64>,
    envelope: AABB<[f64; 2]>,
}

impl CoastlineSegment {
    pub fn new(line: LineString<f64>) -> Self {
        let (min, max) = line_bounding_rect(&line);
        let envelope = AABB::from_corners(min, max);
        Self { line, envelope }
    }
}

impl RTreeObject for CoastlineSegment {
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
        for lp in self.tree.locate_in_envelope_intersecting(&envelope) {
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
        let (min, max) = bounding_rect(poly);
        let envelope = AABB::from_corners(min, max);
        for lp in self.tree.locate_in_envelope_intersecting(&envelope) {
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
        let (min, max) = bounding_rect(poly);
        let envelope = AABB::from_corners(min, max);
        for lp in self.tree.locate_in_envelope_intersecting(&envelope) {
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
        let water_envelope = water_polygons.iter().map(bounding_rect).fold(
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
                    .locate_in_envelope_intersecting(&lp.envelope)
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

/// Spatial index for coastline segments.
pub struct CoastlineIndex {
    tree: RTree<CoastlineSegment>,
}

impl CoastlineIndex {
    pub fn new(segments: Vec<CoastlineSegment>) -> Self {
        let tree = RTree::bulk_load(segments);
        Self { tree }
    }

    pub fn from_serialized(coords: &[Vec<(f32, f32)>]) -> Self {
        let segments: Vec<CoastlineSegment> = coords
            .iter()
            .map(|seg| {
                let line = LineString::from(
                    seg.iter()
                        .map(|&(lon, lat)| Coord {
                            x: lon as f64,
                            y: lat as f64,
                        })
                        .collect::<Vec<_>>(),
                );
                CoastlineSegment::new(line)
            })
            .collect();
        Self::new(segments)
    }

    /// Check if a line segment from (lon1, lat1) to (lon2, lat2) crosses any coastline.
    ///
    /// Antimeridian-aware: a query segment crossing lon +/-180 (e.g. Auckland to
    /// Honolulu) is split into two sub-segments at the seam before testing, so the
    /// R-tree envelope stays local to the segment's actual path instead of spanning
    /// nearly the whole planet, and the planar intersection test no longer runs the
    /// "long way round" through the opposite side of the world.
    ///
    /// Stored coastline segments themselves are short OSM-derived chunks (see
    /// `asw-build/src/coastline.rs::subdivide_ring`) sourced from the pre-split
    /// `land-polygons-split-4326` dataset, which does not emit rings straddling the
    /// seam, so individual stored segments are assumed not to cross it.
    pub fn crosses_land(&self, lon1: f64, lat1: f64, lon2: f64, lat2: f64) -> bool {
        if (lon1 - lon2).abs() > 180.0 {
            let (a, b) = split_at_antimeridian(lon1, lat1, lon2, lat2);
            return self.crosses_land_planar(a.0, a.1, a.2, a.3)
                || self.crosses_land_planar(b.0, b.1, b.2, b.3);
        }
        self.crosses_land_planar(lon1, lat1, lon2, lat2)
    }

    fn crosses_land_planar(&self, lon1: f64, lat1: f64, lon2: f64, lat2: f64) -> bool {
        let min_lon = lon1.min(lon2);
        let max_lon = lon1.max(lon2);
        let min_lat = lat1.min(lat2);
        let max_lat = lat1.max(lat2);
        let envelope = AABB::from_corners([min_lon, min_lat], [max_lon, max_lat]);

        let line = Line::new(Coord { x: lon1, y: lat1 }, Coord { x: lon2, y: lat2 });

        for seg in self.tree.locate_in_envelope_intersecting(&envelope) {
            if line.intersects(&seg.line) {
                return true;
            }
        }
        false
    }

    /// Approximate minimum distance (in degrees) from (lon, lat) to any coastline
    /// segment within `radius_deg`. Returns f64::MAX if nothing found.
    pub fn min_distance_deg(&self, lon: f64, lat: f64, radius_deg: f64) -> f64 {
        let envelope = AABB::from_corners(
            [lon - radius_deg, lat - radius_deg],
            [lon + radius_deg, lat + radius_deg],
        );
        let pt = Coord { x: lon, y: lat };
        let mut best = f64::MAX;

        for seg in self.tree.locate_in_envelope_intersecting(&envelope) {
            for line in seg.line.lines() {
                let d = point_to_segment_dist(pt, line.start, line.end);
                if d < best {
                    best = d;
                }
            }
        }
        best
    }

    pub fn segment_count(&self) -> usize {
        self.tree.size()
    }
}

/// A segment endpoint pair as (lon1, lat1, lon2, lat2).
type LonLatSegment = (f64, f64, f64, f64);

/// Split a query segment that crosses the antimeridian into two sub-segments that
/// meet at the seam (lon = +-180), each expressed in a single consistent longitude
/// frame. Only valid when `(lon1 - lon2).abs() > 180.0`.
fn split_at_antimeridian(
    lon1: f64,
    lat1: f64,
    lon2: f64,
    lat2: f64,
) -> (LonLatSegment, LonLatSegment) {
    // Unwrap into a continuous frame by shifting whichever endpoint is negative,
    // then find where the continuous chord crosses lon = 180.
    let (u1, u2) = if lon1 < 0.0 {
        (lon1 + 360.0, lon2)
    } else {
        (lon1, lon2 + 360.0)
    };
    let t = (180.0 - u1) / (u2 - u1);
    let lat_cross = lat1 + t * (lat2 - lat1);

    let seam1 = if lon1 < 0.0 { -180.0 } else { 180.0 };
    let seam2 = if lon2 < 0.0 { -180.0 } else { 180.0 };

    (
        (lon1, lat1, seam1, lat_cross),
        (seam2, lat_cross, lon2, lat2),
    )
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

fn bounding_rect(poly: &Polygon<f64>) -> ([f64; 2], [f64; 2]) {
    let mut min_x = f64::MAX;
    let mut min_y = f64::MAX;
    let mut max_x = f64::MIN;
    let mut max_y = f64::MIN;
    for coord in poly.exterior().coords() {
        min_x = min_x.min(coord.x);
        min_y = min_y.min(coord.y);
        max_x = max_x.max(coord.x);
        max_y = max_y.max(coord.y);
    }
    ([min_x, min_y], [max_x, max_y])
}

/// Distance from point `p` to the closest point on segment `a`-`b` (in coordinate units).
fn point_to_segment_dist(p: Coord<f64>, a: Coord<f64>, b: Coord<f64>) -> f64 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let len_sq = dx * dx + dy * dy;
    if len_sq == 0.0 {
        // Degenerate segment
        let ex = p.x - a.x;
        let ey = p.y - a.y;
        return (ex * ex + ey * ey).sqrt();
    }
    let t = ((p.x - a.x) * dx + (p.y - a.y) * dy) / len_sq;
    let t = t.clamp(0.0, 1.0);
    let proj_x = a.x + t * dx;
    let proj_y = a.y + t * dy;
    let ex = p.x - proj_x;
    let ey = p.y - proj_y;
    (ex * ex + ey * ey).sqrt()
}

fn line_bounding_rect(ls: &LineString<f64>) -> ([f64; 2], [f64; 2]) {
    let mut min_x = f64::MAX;
    let mut min_y = f64::MAX;
    let mut max_x = f64::MIN;
    let mut max_y = f64::MIN;
    for coord in ls.coords() {
        min_x = min_x.min(coord.x);
        min_y = min_y.min(coord.y);
        max_x = max_x.max(coord.x);
        max_y = max_y.max(coord.y);
    }
    ([min_x, min_y], [max_x, max_y])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line_of(coords: &[(f64, f64)]) -> LineString<f64> {
        LineString::from(
            coords
                .iter()
                .map(|&(x, y)| Coord { x, y })
                .collect::<Vec<_>>(),
        )
    }

    /// A short all-water segment hugging the antimeridian (lat 0, lon 179.9 -> -179.9)
    /// must not be reported as crossing land just because a coastline segment exists
    /// on the far side of the planet. Pre-fix, `crosses_land` builds a planar Line
    /// straight from 179.9 to -179.9, which numerically passes through lon 0 — so it
    /// spuriously intersects a coastline segment sitting in the Gulf of Guinea.
    #[test]
    fn crosses_land_antimeridian_no_false_positive_from_far_land() {
        let far_segment = CoastlineSegment::new(line_of(&[(0.0, -1.0), (0.0, 1.0)]));
        let index = CoastlineIndex::new(vec![far_segment]);
        assert!(
            !index.crosses_land(179.9, 0.0, -179.9, 0.0),
            "seam-hugging water segment must not report a land crossing from unrelated \
             coastline on the far side of the planet"
        );
    }

    /// A coastline segment that actually straddles the seam-crossing path, right at the
    /// antimeridian, must be detected. Pre-fix, the planar Line's endpoints only span
    /// lon in [-179.9, 179.9], so a coastline segment sitting just outside that range
    /// (e.g. at lon 179.95) is never tested against and the crossing is missed.
    #[test]
    fn crosses_land_antimeridian_detects_real_crossing_near_seam() {
        let near_segment = CoastlineSegment::new(line_of(&[(179.95, -1.0), (179.95, 1.0)]));
        let index = CoastlineIndex::new(vec![near_segment]);
        assert!(
            index.crosses_land(179.9, 0.0, -179.9, 0.0),
            "a coastline segment actually straddling the seam-crossing path must be detected"
        );
    }

    #[test]
    fn crosses_land_normal_case_unaffected() {
        // A segment nowhere near the antimeridian, crossing a coastline segment that
        // actually blocks it, must still be detected (sanity check, no regression).
        let blocking = CoastlineSegment::new(line_of(&[(10.0, -1.0), (10.0, 1.0)]));
        let index = CoastlineIndex::new(vec![blocking]);
        assert!(index.crosses_land(5.0, 0.0, 15.0, 0.0));
        assert!(!index.crosses_land(5.0, 0.0, 5.0, 1.0));
    }

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

    #[test]
    fn min_distance_deg_matches_expected_point_to_segment_distance() {
        // A simple L-shaped coastline; point-to-segment distance is well known here.
        let seg = line_of(&[(0.0, 0.0), (0.0, 1.0), (1.0, 1.0)]);
        let index = CoastlineIndex::new(vec![CoastlineSegment::new(seg)]);

        // Directly on the line -> distance ~0.
        let on_line = index.min_distance_deg(0.0, 0.5, 5.0);
        assert!(on_line < 1e-9, "expected ~0, got {on_line}");

        // Perpendicular distance of 0.5 deg from the vertical segment.
        let off_line = index.min_distance_deg(0.5, 0.5, 5.0);
        assert!(
            (off_line - 0.5).abs() < 1e-9,
            "expected ~0.5, got {off_line}"
        );

        // Nothing within a tiny radius far away.
        let far = index.min_distance_deg(50.0, 50.0, 0.5);
        assert_eq!(far, f64::MAX);
    }
}
