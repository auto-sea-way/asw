//! Coastline stored as i32 microdegree runs plus a 0.1° grid index over run
//! bounding boxes. `CoastlineSections` is the build-side owner of the five
//! arrays; `CoastlineIndex` is a borrowed view over them (from a
//! `CoastlineSections` in tests and the build, from the mapped file at
//! query time) and answers the router's geometry questions.

use geo::{Coord, Intersects, Line};

pub const GRID_STEP_DEG: f64 = 0.1;
pub const GRID_COLS: usize = 3600;
pub const GRID_ROWS: usize = 1800;
pub const GRID_CELLS: usize = GRID_COLS * GRID_ROWS;
const MICRO: f64 = 1e6;

fn micro(deg: f64) -> i32 {
    (deg * MICRO).round() as i32
}

fn grid_col(lon: f64) -> usize {
    ((lon + 180.0) / GRID_STEP_DEG)
        .floor()
        .clamp(0.0, (GRID_COLS - 1) as f64) as usize
}

fn grid_row(lat: f64) -> usize {
    ((lat + 90.0) / GRID_STEP_DEG)
        .floor()
        .clamp(0.0, (GRID_ROWS - 1) as f64) as usize
}

/// The coastline sections exactly as written to a v4 graph file.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CoastlineSections {
    /// Point index where run `i` starts. Length = run count + 1.
    pub runs: Vec<u32>,
    /// Per run: min_lon, min_lat, max_lon, max_lat in microdegrees.
    pub bbox: Vec<i32>,
    /// Interleaved lon, lat in microdegrees.
    pub points: Vec<i32>,
    /// CSR offsets into `grid_ids`, one per grid cell plus a sentinel.
    pub grid_offsets: Vec<u32>,
    /// Run ids whose bbox touches the cell.
    pub grid_ids: Vec<u32>,
}

impl CoastlineSections {
    /// Runs are (lon, lat) polylines in degrees. Runs with fewer than two
    /// points are dropped.
    pub fn from_runs(runs: &[Vec<(f64, f64)>]) -> Self {
        let mut out = Self {
            runs: vec![0],
            ..Self::default()
        };
        let mut cell_run: Vec<(u32, u32)> = Vec::new();
        for run in runs.iter().filter(|r| r.len() >= 2) {
            let id = (out.runs.len() - 1) as u32;
            let (mut min_lon, mut min_lat) = (f64::MAX, f64::MAX);
            let (mut max_lon, mut max_lat) = (f64::MIN, f64::MIN);
            for &(lon, lat) in run {
                out.points.push(micro(lon));
                out.points.push(micro(lat));
                min_lon = min_lon.min(lon);
                max_lon = max_lon.max(lon);
                min_lat = min_lat.min(lat);
                max_lat = max_lat.max(lat);
            }
            out.runs.push((out.points.len() / 2) as u32);
            out.bbox.extend([
                micro(min_lon),
                micro(min_lat),
                micro(max_lon),
                micro(max_lat),
            ]);
            for row in grid_row(min_lat)..=grid_row(max_lat) {
                for col in grid_col(min_lon)..=grid_col(max_lon) {
                    cell_run.push(((row * GRID_COLS + col) as u32, id));
                }
            }
        }
        cell_run.sort_unstable();
        out.grid_offsets = vec![0u32; GRID_CELLS + 1];
        for &(cell, _) in &cell_run {
            out.grid_offsets[cell as usize + 1] += 1;
        }
        for i in 0..GRID_CELLS {
            out.grid_offsets[i + 1] += out.grid_offsets[i];
        }
        out.grid_ids = cell_run.into_iter().map(|(_, id)| id).collect();
        out
    }

    pub fn index(&self) -> CoastlineIndex<'_> {
        CoastlineIndex::from_slices(
            &self.runs,
            &self.bbox,
            &self.points,
            &self.grid_offsets,
            &self.grid_ids,
        )
    }
}

/// Borrowed view over the coastline sections.
#[derive(Clone, Copy)]
pub struct CoastlineIndex<'a> {
    runs: &'a [u32],
    bbox: &'a [i32],
    points: &'a [i32],
    grid_offsets: &'a [u32],
    grid_ids: &'a [u32],
}

impl<'a> CoastlineIndex<'a> {
    pub fn from_slices(
        runs: &'a [u32],
        bbox: &'a [i32],
        points: &'a [i32],
        grid_offsets: &'a [u32],
        grid_ids: &'a [u32],
    ) -> Self {
        Self {
            runs,
            bbox,
            points,
            grid_offsets,
            grid_ids,
        }
    }

    pub fn run_count(&self) -> usize {
        self.runs.len().saturating_sub(1)
    }

    /// Points of one run as (lon, lat) degrees.
    pub fn run_points(&self, run: usize) -> impl Iterator<Item = (f64, f64)> + 'a {
        let (s, e) = (self.runs[run] as usize * 2, self.runs[run + 1] as usize * 2);
        self.points[s..e]
            .chunks_exact(2)
            .map(|p| (p[0] as f64 / MICRO, p[1] as f64 / MICRO))
    }

    fn lines(&self, run: usize) -> impl Iterator<Item = Line<f64>> + 'a {
        let (s, e) = (self.runs[run] as usize * 2, self.runs[run + 1] as usize * 2);
        self.points[s..e].windows(4).step_by(2).map(|w| {
            Line::new(
                Coord {
                    x: w[0] as f64 / MICRO,
                    y: w[1] as f64 / MICRO,
                },
                Coord {
                    x: w[2] as f64 / MICRO,
                    y: w[3] as f64 / MICRO,
                },
            )
        })
    }

    /// Sorted, deduplicated ids of runs whose bbox intersects the envelope
    /// (degrees; may overflow ±180 after a wrap retry, the grid clamps).
    fn candidates(&self, min_lon: f64, min_lat: f64, max_lon: f64, max_lat: f64) -> Vec<usize> {
        let (mn_lon, mn_lat, mx_lon, mx_lat) = (
            micro(min_lon),
            micro(min_lat),
            micro(max_lon),
            micro(max_lat),
        );
        let mut ids = Vec::new();
        for row in grid_row(min_lat)..=grid_row(max_lat) {
            for col in grid_col(min_lon)..=grid_col(max_lon) {
                let cell = row * GRID_COLS + col;
                let (s, e) = (
                    self.grid_offsets[cell] as usize,
                    self.grid_offsets[cell + 1] as usize,
                );
                for &id in &self.grid_ids[s..e] {
                    let b = &self.bbox[id as usize * 4..id as usize * 4 + 4];
                    if b[0] > mx_lon || b[2] < mn_lon || b[1] > mx_lat || b[3] < mn_lat {
                        continue;
                    }
                    ids.push(id as usize);
                }
            }
        }
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    /// Does the segment cross any coastline? Antimeridian-aware: a query
    /// crossing lon ±180 is split at the seam first.
    pub fn crosses_land(&self, lon1: f64, lat1: f64, lon2: f64, lat2: f64) -> bool {
        if (lon1 - lon2).abs() > 180.0 {
            let (a, b) = split_at_antimeridian(lon1, lat1, lon2, lat2);
            return self.crosses_land_planar(a.0, a.1, a.2, a.3)
                || self.crosses_land_planar(b.0, b.1, b.2, b.3);
        }
        self.crosses_land_planar(lon1, lat1, lon2, lat2)
    }

    fn crosses_land_planar(&self, lon1: f64, lat1: f64, lon2: f64, lat2: f64) -> bool {
        let line = Line::new(Coord { x: lon1, y: lat1 }, Coord { x: lon2, y: lat2 });
        self.candidates(
            lon1.min(lon2),
            lat1.min(lat2),
            lon1.max(lon2),
            lat1.max(lat2),
        )
        .into_iter()
        .any(|run| self.lines(run).any(|l| line.intersects(&l)))
    }

    /// Number of coastline edges the segment P->T crosses, half-open so a
    /// shared vertex on the line counts once and a touch counts zero or
    /// two. Parity from a known-water P tells whether T is on water.
    pub fn crossing_count(&self, lon1: f64, lat1: f64, lon2: f64, lat2: f64) -> usize {
        if (lon1 - lon2).abs() > 180.0 {
            let (a, b) = split_at_antimeridian(lon1, lat1, lon2, lat2);
            return self.crossing_count_planar(a.0, a.1, a.2, a.3)
                + self.crossing_count_planar(b.0, b.1, b.2, b.3);
        }
        self.crossing_count_planar(lon1, lat1, lon2, lat2)
    }

    fn crossing_count_planar(&self, px: f64, py: f64, tx: f64, ty: f64) -> usize {
        let (dx, dy) = (tx - px, ty - py);
        // Strictly left of P->T; a point on the line counts as right.
        let left = |x: f64, y: f64| dx * (y - py) - dy * (x - px) > 0.0;
        let mut n = 0;
        for run in self.candidates(px.min(tx), py.min(ty), px.max(tx), py.max(ty)) {
            for l in self.lines(run) {
                let (a, b) = (l.start, l.end);
                if left(a.x, a.y) == left(b.x, b.y) {
                    continue;
                }
                let (ex, ey) = (b.x - a.x, b.y - a.y);
                let denom = dx * ey - dy * ex;
                if denom == 0.0 {
                    continue;
                }
                let t = ((a.x - px) * ey - (a.y - py) * ex) / denom;
                if (0.0..1.0).contains(&t) {
                    n += 1;
                }
            }
        }
        n
    }

    /// Minimum distance in degrees within `radius_deg`, `f64::MAX` if none.
    pub fn min_distance_deg(&self, lon: f64, lat: f64, radius_deg: f64) -> f64 {
        with_wrap_retry(lon, lon, radius_deg, |shift| {
            self.min_distance_deg_planar(lon + shift, lat, radius_deg)
        })
    }

    fn min_distance_deg_planar(&self, lon: f64, lat: f64, radius_deg: f64) -> f64 {
        let pt = Coord { x: lon, y: lat };
        let mut best = f64::MAX;
        for run in self.candidates(
            lon - radius_deg,
            lat - radius_deg,
            lon + radius_deg,
            lat + radius_deg,
        ) {
            for l in self.lines(run) {
                best = best.min(point_to_segment_dist(pt, l.start, l.end));
            }
        }
        best
    }

    /// Minimum distance in nm, capped at `max_nm`.
    pub fn min_distance_nm(&self, lon: f64, lat: f64, max_nm: f64) -> f64 {
        let coslat = cos_lat_clamped(lat);
        let dlon = nm_lon_radius(max_nm, coslat);
        with_wrap_retry(lon, lon, dlon, |shift| {
            self.min_distance_nm_planar(lon + shift, lat, max_nm, coslat)
        })
    }

    fn min_distance_nm_planar(&self, lon: f64, lat: f64, max_nm: f64, coslat: f64) -> f64 {
        let dlat = max_nm / 60.0;
        let dlon = max_nm / (60.0 * coslat);
        let origin = Coord { x: 0.0, y: 0.0 };
        let mut best = max_nm;
        for run in self.candidates(lon - dlon, lat - dlat, lon + dlon, lat + dlat) {
            for l in self.lines(run) {
                let a = nm_frame(l.start, lon, lat, coslat);
                let b = nm_frame(l.end, lon, lat, coslat);
                best = best.min(point_to_segment_dist(origin, a, b));
            }
        }
        best
    }

    /// Minimum distance in nm from a query segment to the coastline, capped
    /// at `max_nm`; 0.0 on intersection.
    pub fn segment_min_distance_nm(
        &self,
        lon1: f64,
        lat1: f64,
        lon2: f64,
        lat2: f64,
        max_nm: f64,
    ) -> f64 {
        if (lon1 - lon2).abs() > 180.0 {
            let (a, b) = split_at_antimeridian(lon1, lat1, lon2, lat2);
            return self
                .segment_min_distance_nm_wrapped(a.0, a.1, a.2, a.3, max_nm)
                .min(self.segment_min_distance_nm_wrapped(b.0, b.1, b.2, b.3, max_nm));
        }
        self.segment_min_distance_nm_wrapped(lon1, lat1, lon2, lat2, max_nm)
    }

    fn segment_min_distance_nm_wrapped(
        &self,
        lon1: f64,
        lat1: f64,
        lon2: f64,
        lat2: f64,
        max_nm: f64,
    ) -> f64 {
        let coslat = cos_lat_clamped((lat1 + lat2) / 2.0);
        let dlon = nm_lon_radius(max_nm, coslat);
        with_wrap_retry(lon1.min(lon2), lon1.max(lon2), dlon, |shift| {
            self.segment_min_distance_nm_planar(lon1 + shift, lat1, lon2 + shift, lat2, max_nm)
        })
    }

    fn segment_min_distance_nm_planar(
        &self,
        lon1: f64,
        lat1: f64,
        lon2: f64,
        lat2: f64,
        max_nm: f64,
    ) -> f64 {
        let ref_lat = (lat1 + lat2) / 2.0;
        let coslat = cos_lat_clamped(ref_lat);
        let dlat = max_nm / 60.0;
        let dlon = max_nm / (60.0 * coslat);
        let qa = nm_frame(Coord { x: lon1, y: lat1 }, lon1, ref_lat, coslat);
        let qb = nm_frame(Coord { x: lon2, y: lat2 }, lon1, ref_lat, coslat);
        let query = Line::new(qa, qb);
        let mut best = max_nm;
        for run in self.candidates(
            lon1.min(lon2) - dlon,
            lat1.min(lat2) - dlat,
            lon1.max(lon2) + dlon,
            lat1.max(lat2) + dlat,
        ) {
            for l in self.lines(run) {
                let ca = nm_frame(l.start, lon1, ref_lat, coslat);
                let cb = nm_frame(l.end, lon1, ref_lat, coslat);
                if query.intersects(&Line::new(ca, cb)) {
                    return 0.0;
                }
                let d = point_to_segment_dist(qa, ca, cb)
                    .min(point_to_segment_dist(qb, ca, cb))
                    .min(point_to_segment_dist(ca, qa, qb))
                    .min(point_to_segment_dist(cb, qa, qb));
                best = best.min(d);
            }
        }
        best
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

/// cos(lat) clamped away from zero for degree->nm longitude scaling near poles.
fn cos_lat_clamped(lat: f64) -> f64 {
    lat.to_radians().cos().max(0.01)
}

/// Longitude half-width (in degrees) of a `max_nm` search radius at `coslat`.
fn nm_lon_radius(max_nm: f64, coslat: f64) -> f64 {
    max_nm / (60.0 * coslat)
}

/// Run `query(0.0)` for the primary (non-wrapped) frame and, if the query's
/// longitude extent `[lon_min, lon_max]` expanded by `dlon` overflows past
/// +/-180, retry `query` shifted by the opposite full turn (-360 or +360) and
/// take the minimum. This is the shared antimeridian handling used by both
/// the point (`min_distance_nm`/`min_distance_deg`) and segment
/// (`segment_min_distance_nm_wrapped`) planar distance queries: stored
/// coastline segments always live within [-180, 180], so shifting the query
/// by a full turn puts it in the same frame as segments on the far side of
/// the seam without needing to touch the stored data.
fn with_wrap_retry(
    lon_min: f64,
    lon_max: f64,
    dlon: f64,
    mut query: impl FnMut(f64) -> f64,
) -> f64 {
    let mut best = query(0.0);
    if lon_max + dlon > 180.0 {
        best = best.min(query(-360.0));
    } else if lon_min - dlon < -180.0 {
        best = best.min(query(360.0));
    }
    best
}

/// Project a lon/lat coordinate into a local equirectangular nm frame
/// centered on (ref_lon, ref_lat). Exact enough at <= ~5 nm scale.
fn nm_frame(c: Coord<f64>, ref_lon: f64, ref_lat: f64, coslat: f64) -> Coord<f64> {
    Coord {
        x: (c.x - ref_lon) * 60.0 * coslat,
        y: (c.y - ref_lat) * 60.0,
    }
}

/// Distance from point `p` to the closest point on segment `a`-`b` (in coordinate units).
///
/// Hand-rolled on purpose: geo's `Euclidean.distance(coord, &line)` computes
/// lengths via `hypot` (overflow-safe but several times slower than plain
/// `sqrt`), and this runs in the innermost loop of every coastline distance
/// query — swapping it in measured +9-30% p50 on short-route benches.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn wall() -> CoastlineSections {
        CoastlineSections::from_runs(&[vec![(10.0, -1.0), (10.0, 1.0)]])
    }

    #[test]
    fn from_runs_encodes_points_bbox_and_grid() {
        let s = CoastlineSections::from_runs(&[
            vec![(10.0, -1.0), (10.0, 1.0)],
            vec![(0.5, 0.5)], // dropped: fewer than 2 points
            vec![(-0.05, 0.0), (0.05, 0.0)],
        ]);
        assert_eq!(s.runs, vec![0, 2, 4]);
        assert_eq!(
            s.points,
            vec![10_000_000, -1_000_000, 10_000_000, 1_000_000, -50_000, 0, 50_000, 0]
        );
        assert_eq!(
            &s.bbox[0..4],
            &[10_000_000, -1_000_000, 10_000_000, 1_000_000]
        );
        assert_eq!(s.grid_offsets.len(), GRID_CELLS + 1);
        assert_eq!(*s.grid_offsets.last().unwrap() as usize, s.grid_ids.len());
        // The wall spans 21 lat rows (lat -1.0..=1.0 at 0.1°) in one lon column.
        assert_eq!(s.grid_ids.iter().filter(|&&id| id == 0).count(), 21);
        // The short run straddles lon 0 (cols 1799 and 1800), one row.
        assert_eq!(s.grid_ids.iter().filter(|&&id| id == 1).count(), 2);
    }

    #[test]
    fn empty_sections_answer_every_query_as_open_water() {
        let s = CoastlineSections::from_runs(&[]);
        let idx = s.index();
        assert_eq!(idx.run_count(), 0);
        assert!(!idx.crosses_land(0.0, 0.0, 1.0, 1.0));
        assert_eq!(idx.crossing_count(0.0, 0.0, 1.0, 1.0), 0);
        assert_eq!(idx.min_distance_nm(28.0, 36.5, 5.1), 5.1);
        assert_eq!(idx.min_distance_deg(0.0, 0.0, 1.0), f64::MAX);
    }

    #[test]
    fn crosses_land_normal_case() {
        let s = wall();
        let idx = s.index();
        assert!(idx.crosses_land(5.0, 0.0, 15.0, 0.0));
        assert!(!idx.crosses_land(5.0, 0.0, 5.0, 1.0));
    }

    #[test]
    fn crosses_land_antimeridian_no_false_positive_from_far_land() {
        let s = CoastlineSections::from_runs(&[vec![(0.0, -1.0), (0.0, 1.0)]]);
        assert!(!s.index().crosses_land(179.9, 0.0, -179.9, 0.0));
    }

    #[test]
    fn crosses_land_antimeridian_detects_real_crossing_near_seam() {
        let s = CoastlineSections::from_runs(&[vec![(179.95, -1.0), (179.95, 1.0)]]);
        assert!(s.index().crosses_land(179.9, 0.0, -179.9, 0.0));
    }

    #[test]
    fn grid_clamps_envelope_beyond_the_seam() {
        // A query shifted by +360 (wrap retry) must not index outside the grid.
        let s = CoastlineSections::from_runs(&[vec![(179.98, -0.5), (179.98, 0.5)]]);
        let d = s.index().min_distance_deg(-179.99, 0.0, 0.05);
        assert!(
            (d - 0.03).abs() < 1e-9,
            "expected ~0.03 deg across the seam, got {d}"
        );
        let d2 = s.index().min_distance_deg(-179.99 + 360.0, 0.0, 0.05);
        assert!(d2.is_finite());
    }

    #[test]
    fn min_distance_deg_matches_point_to_segment_distance() {
        let s = CoastlineSections::from_runs(&[vec![(0.0, 0.0), (0.0, 1.0), (1.0, 1.0)]]);
        let idx = s.index();
        assert!(idx.min_distance_deg(0.0, 0.5, 5.0) < 1e-9);
        assert!((idx.min_distance_deg(0.5, 0.5, 5.0) - 0.5).abs() < 1e-9);
        assert_eq!(idx.min_distance_deg(50.0, 50.0, 0.5), f64::MAX);
    }

    #[test]
    fn min_distance_nm_mid_latitude_and_cap() {
        let s = CoastlineSections::from_runs(&[vec![(28.0, 36.0), (28.0, 37.0)]]);
        let idx = s.index();
        let expected = 0.1 * 60.0 * (36.5f64).to_radians().cos();
        let d = idx.min_distance_nm(28.1, 36.5, 5.1);
        assert!((d - expected).abs() < 0.05, "got {d}, expected {expected}");
        assert_eq!(idx.min_distance_nm(29.0, 36.5, 5.1), 5.1);
    }

    #[test]
    fn segment_min_distance_nm_parallel_and_crossing() {
        let s = CoastlineSections::from_runs(&[vec![(28.0, 36.0), (28.0, 37.0)]]);
        let idx = s.index();
        let expected = 0.05 * 60.0 * (36.5f64).to_radians().cos();
        let d = idx.segment_min_distance_nm(28.05, 36.4, 28.05, 36.6, 5.1);
        assert!((d - expected).abs() < 0.05, "got {d}, expected {expected}");
        assert_eq!(
            idx.segment_min_distance_nm(27.9, 36.5, 28.1, 36.5, 5.1),
            0.0
        );
    }

    #[test]
    fn distances_across_antimeridian() {
        let s = CoastlineSections::from_runs(&[vec![(179.98, -0.5), (179.98, 0.5)]]);
        let d = s.index().min_distance_nm(-179.99, 0.0, 5.1);
        assert!((d - 1.8).abs() < 0.05, "got {d}, expected 1.8");
        let s2 = CoastlineSections::from_runs(&[vec![(179.98, 0.05), (179.98, 0.2)]]);
        let d2 = s2
            .index()
            .segment_min_distance_nm(179.9, 0.0, -179.9, 0.0, 5.1);
        assert!((d2 - 3.0).abs() < 0.05, "got {d2}, expected 3.0");
    }

    /// Diamond island around the origin; the query line y=0 passes exactly
    /// through its west vertex. The half-open rule must count that once.
    fn diamond() -> CoastlineSections {
        CoastlineSections::from_runs(&[vec![
            (-0.1, 0.0),
            (0.0, -0.1),
            (0.1, 0.0),
            (0.0, 0.1),
            (-0.1, 0.0),
        ]])
    }

    #[test]
    fn crossing_count_parity_inside_and_through_island() {
        let s = diamond();
        let idx = s.index();
        assert_eq!(
            idx.crossing_count(-1.0, 0.0, 0.0, 0.0),
            1,
            "into the island: odd"
        );
        assert_eq!(
            idx.crossing_count(-1.0, 0.0, 1.0, 0.0),
            2,
            "through the island: even"
        );
        assert_eq!(
            idx.crossing_count(-1.0, 0.5, 1.0, 0.5),
            0,
            "misses the island"
        );
    }

    #[test]
    fn crossing_count_touching_vertex_is_even() {
        // y = 0.1 touches the north vertex without entering.
        let s = diamond();
        assert_eq!(s.index().crossing_count(-1.0, 0.1, 1.0, 0.1) % 2, 0);
    }

    #[test]
    fn crossing_count_across_antimeridian() {
        let s = CoastlineSections::from_runs(&[vec![(179.95, -1.0), (179.95, 1.0)]]);
        assert_eq!(s.index().crossing_count(179.9, 0.0, -179.9, 0.0), 1);
    }
}
