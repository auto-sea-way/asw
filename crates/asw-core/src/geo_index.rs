use geo::{Contains, Coord, Intersects, Line, LineString, Point, Polygon};
use rstar::{RTree, RTreeObject, AABB};

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
    pub fn intersects_polygon(&self, poly: &Polygon<f64>) -> bool {
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
    pub fn contains_polygon(&self, poly: &Polygon<f64>) -> bool {
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
    pub fn crosses_land(&self, lon1: f64, lat1: f64, lon2: f64, lat2: f64) -> bool {
        let min_lon = lon1.min(lon2);
        let max_lon = lon1.max(lon2);
        let min_lat = lat1.min(lat2);
        let max_lat = lat1.max(lat2);
        let envelope = AABB::from_corners([min_lon, min_lat], [max_lon, max_lat]);

        let line = Line::new(
            Coord { x: lon1, y: lat1 },
            Coord { x: lon2, y: lat2 },
        );

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
            let coords: Vec<_> = seg.line.coords().collect();
            for w in coords.windows(2) {
                let d = point_to_segment_dist(pt, *w[0], *w[1]);
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
