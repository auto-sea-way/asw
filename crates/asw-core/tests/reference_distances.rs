//! Planet-graph routes checked against sources that do not come from the
//! engine or from OpenStreetMap.
//!
//! 1. Distance: NGA Pub 151, Distances Between Ports (11th ed. 2001,
//!    corrected through NM 14/2004),
//!    https://msi.nga.mil/Publications/Distances-Between-Ports. Positions
//!    are the port and junction-point positions printed in Pub 151. Only
//!    pairs whose Pub 151 track is a shortest navigable route are used: no
//!    traffic-lane tracks (New York to northern Europe runs south of the
//!    Nantucket lanes, about 3 % longer) and no ports up a river the graph
//!    has no water for (Rotterdam, Shanghai).
//! 2. Land: Natural Earth 1:10m land polygons, v5.1.1, public domain,
//!    https://www.naturalearthdata.com/downloads/10m-physical-vectors/10m-land/.
//!    Every water leg is walked along its great circle; no point may be
//!    deep inside land (issue #65: a great circle through Mexico).
//!
//! Needs the planet graph and the land shapefile, so it is ignored by
//! default. Paths default to export/asw.graph and
//! export/ne_10m_land/ne_10m_land.shp; ASW_GRAPH and ASW_CHECK_LAND override:
//! cargo test --release -p asw-core --test reference_distances -- --ignored --nocapture

use asw_core::astar_pool::AstarBuffers;
use asw_core::graph::RoutingGraph;
use asw_core::h3::haversine_nm;
use asw_core::passages::PASSAGES;
use asw_core::routing::{compute_route, LEG_STEP_NM};
use geo::{BoundingRect, Contains, MultiPolygon, Point, Polygon, Rect};
use std::path::{Path, PathBuf};

/// (name, from_lat, from_lon, to_lat, to_lon, Pub 151 nm)
#[rustfmt::skip]
const PUB151: &[(&str, f64, f64, f64, f64, f64)] = &[
    ("Ouessant - Strait of Gibraltar",   48.6667,   -5.5,      35.95,    -5.75,     918.0),
    ("Strait of Gibraltar - Port Said",  35.95,     -5.75,     31.2667,  32.3167,  1943.0),
    ("Port Said - Singapore",            31.2667,   32.3167,    1.2667, 103.8333,  5035.0),
    ("Cape Town - Rio de Janeiro",      -33.9,      18.4333,  -22.8917, -43.1833,  3273.0),
    ("Sydney - Auckland",               -33.8583,  151.2167,  -36.8333, 174.7833,  1280.0),
    ("Sydney - Wellington",             -33.8583,  151.2167,  -41.2833, 174.7833,  1234.0),
    ("Panama - Yokohama",                 8.8833,  -79.5167,   35.45,   139.65,    7682.0),
    ("Strait of Gibraltar - New York",   35.95,     -5.75,     40.7,    -74.0167,  3176.0),
    ("Lisboa - Rio de Janeiro",          38.7,      -9.1333,  -22.8917, -43.1833,  4219.0),
    ("Honolulu - Los Angeles",           21.3083, -157.8708,   33.75,  -118.2708,  2228.0),
    ("Singapore - Colombo",               1.2667,  103.8333,    6.95,    79.85,    1581.0),
    ("Colombo - Port Said",               6.95,     79.85,     31.2667,  32.3167,  3481.0),
    ("Honolulu - Yokohama",              21.3083, -157.8708,   35.45,   139.65,    3395.0),
    ("Yokohama - San Francisco",         35.45,    139.65,     37.8083, -122.4,    4536.0),
    ("Honolulu - San Francisco",         21.3083, -157.8708,   37.8083, -122.4,    2091.0),
    ("Honolulu - Sydney",                21.3083, -157.8708,  -33.8583, 151.2167,  4420.0),
    ("Panama - Singapore",                8.8833,  -79.5167,    1.2667, 103.8333, 10505.0),
    ("New York - Panama",                40.7,     -74.0167,    8.8833, -79.5167,  2016.0),
    ("Cape of Good Hope - Singapore",   -34.3667,   18.3833,    1.2667, 103.8333,  5579.0),
    ("Cape Town - New York",            -33.9,      18.4333,   40.7,    -74.0167,  6786.0),
    ("Lisboa - New York",                38.7,      -9.1333,   40.7,    -74.0167,  2980.0),
    ("Strait of Gibraltar - Cape Town",  35.95,     -5.75,    -33.9,     18.4333,  5024.0),
];

/// The two routes reported in issue #65: (name, from_lat, from_lon, to_lat,
/// to_lon, shore_buffer_nm). No published distance, land check only.
#[rustfmt::skip]
const ISSUE_65: &[(&str, f64, f64, f64, f64, f64)] = &[
    ("#65 Saint-Nazaire - Bahamas", 47.27, -2.21, 25.9,  -77.0, 0.0),
    ("#65 Miami - 30N 140E",        26.0, -80.0,  30.0,  140.0, 5.0),
];

/// Pub 151 rounds to whole miles and its port positions are central
/// positions, often inland of the coastline the graph uses.
const TOLERANCE: f64 = 0.02;

/// A point is deep inside land when it and the four points this far north,
/// south, east and west of it are all on one Natural Earth land polygon. Natural Earth
/// 1:10m is coarser than OSM (its coast can be off by a kilometre or more),
/// so a route hugging the OSM coast must not count.
const DEEP_NM: f64 = 3.0;
/// Spacing of the land samples along each leg.
const SAMPLE_NM: f64 = 2.0;

fn export_path(env: &str, rel: &str) -> PathBuf {
    std::env::var_os(env).map(PathBuf::from).unwrap_or_else(|| {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../export")
            .join(rel)
    })
}

struct Land(Vec<(Rect, Polygon)>);

impl Land {
    fn load(path: &Path) -> Self {
        let shapes = shapefile::read_shapes_as::<_, shapefile::Polygon>(path)
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let mut polys = Vec::new();
        for shape in shapes {
            for p in MultiPolygon::try_from(shape).unwrap() {
                polys.push((p.bounding_rect().unwrap(), p));
            }
        }
        Land(polys)
    }

    /// Is the point at least `DEEP_NM` inside one land polygon: the point
    /// and the four points that far north, south, east and west of it all
    /// on the same polygon? One polygon, so a channel between islands does
    /// not count as land.
    fn deep(&self, lat: f64, lon: f64) -> bool {
        let dlat = DEEP_NM / 60.0;
        let dlon = DEEP_NM / (60.0 * lat.to_radians().cos().max(0.01));
        let inside = |(r, p): &(Rect, Polygon), lat: f64, lon: f64| {
            lon >= r.min().x
                && lon <= r.max().x
                && lat >= r.min().y
                && lat <= r.max().y
                && p.contains(&Point::new(lon, lat))
        };
        self.0.iter().any(|poly| {
            [
                (0.0, 0.0),
                (dlat, 0.0),
                (-dlat, 0.0),
                (0.0, dlon),
                (0.0, -dlon),
            ]
            .iter()
            .all(|&(a, o)| inside(poly, lat + a, wrap(lon + o)))
        })
    }
}

fn wrap(lon: f64) -> f64 {
    (lon + 540.0).rem_euclid(360.0) - 180.0
}

/// Point at fraction `f` along the great circle, as (lat, lon). Written out
/// here instead of using the engine's own helper so the check stays
/// independent of the code it tests.
fn slerp(lat1: f64, lon1: f64, lat2: f64, lon2: f64, f: f64) -> (f64, f64) {
    let v = |lat: f64, lon: f64| {
        let (la, lo) = (lat.to_radians(), lon.to_radians());
        [la.cos() * lo.cos(), la.cos() * lo.sin(), la.sin()]
    };
    let (a, b) = (v(lat1, lon1), v(lat2, lon2));
    let d = (a[0] * b[0] + a[1] * b[1] + a[2] * b[2])
        .clamp(-1.0, 1.0)
        .acos();
    if d < 1e-12 {
        return (lat1, lon1);
    }
    let (wa, wb) = (((1.0 - f) * d).sin() / d.sin(), (f * d).sin() / d.sin());
    let p: Vec<f64> = (0..3).map(|i| wa * a[i] + wb * b[i]).collect();
    (
        p[2].atan2(p[0].hypot(p[1])).to_degrees(),
        p[1].atan2(p[0]).to_degrees(),
    )
}

fn exempt(lat: f64, lon: f64) -> bool {
    PASSAGES
        .iter()
        .map(|p| p.corridor)
        .any(|(x0, y0, x1, y1)| lon >= x0 && lon <= x1 && lat >= y0 && lat <= y1)
}

/// First point of a water leg that is deep inside Natural Earth land.
fn deep_land_point(
    land: &Land,
    coords: &[[f64; 2]],
    land_legs: &[usize],
) -> Option<(usize, f64, f64)> {
    for (i, w) in coords.windows(2).enumerate() {
        if land_legs.contains(&i) {
            continue;
        }
        let ([lon1, lat1], [lon2, lat2]) = (w[0], w[1]);
        let n = (haversine_nm(lat1, lon1, lat2, lon2) / SAMPLE_NM)
            .ceil()
            .max(1.0) as usize;
        for k in 0..=n {
            let (lat, lon) = slerp(lat1, lon1, lat2, lon2, k as f64 / n as f64);
            if !exempt(lat, lon) && land.deep(lat, lon) {
                return Some((i, lat, lon));
            }
        }
    }
    None
}

#[test]
#[ignore = "needs the planet graph and the Natural Earth land shapefile"]
fn planet_routes_match_pub151_and_stay_off_land() {
    let graph = RoutingGraph::open(&export_path("ASW_GRAPH", "asw.graph"), false).unwrap();
    let land = Land::load(&export_path(
        "ASW_CHECK_LAND",
        "ne_10m_land/ne_10m_land.shp",
    ));
    let coastline = graph.coastline();
    let knn = |lat: f64, lon: f64| graph.nearest_node(lat, lon);
    let mut buffers = AstarBuffers::new(graph.num_nodes() as usize);

    let routes = PUB151
        .iter()
        .map(|&(n, a, b, c, d, nm)| (n, a, b, c, d, 0.0, Some(nm)))
        .chain(
            ISSUE_65
                .iter()
                .map(|&(n, a, b, c, d, buf)| (n, a, b, c, d, buf, None)),
        );

    let mut failures = Vec::new();
    for (name, flat, flon, tlat, tlon, buffer, want) in routes {
        buffers.reset();
        let r = compute_route(
            &graph,
            flat,
            flon,
            tlat,
            tlon,
            &coastline,
            &knn,
            &mut buffers,
            buffer,
            false,
            true,
        )
        .unwrap_or_else(|| panic!("{name}: no route"));

        if let Some((leg, lat, lon)) = deep_land_point(&land, &r.coordinates, &r.land_legs) {
            failures.push(format!(
                "{name}: leg {leg} runs over land at {lat:.3}, {lon:.3}"
            ));
        }

        let mut water = 0.0;
        for (i, w) in r.coordinates.windows(2).enumerate() {
            let leg = haversine_nm(w[0][1], w[0][0], w[1][1], w[1][0]);
            if leg > LEG_STEP_NM + 1e-6 {
                failures.push(format!("{name}: leg {i} is {leg:.1} nm"));
                break;
            }
            if !r.land_legs.contains(&i) {
                water += leg;
            }
        }
        if (r.distance_nm - water).abs() > 1e-6 {
            failures.push(format!(
                "{name}: distance_nm {} is not the polyline length {water}",
                r.distance_nm
            ));
        }

        match want {
            Some(want) => {
                let off = (r.distance_nm - want) / want;
                println!(
                    "{name:34} {:8.1} nm  Pub 151 {want:6.0}  {:+5.1} %",
                    r.distance_nm,
                    off * 100.0
                );
                if off.abs() > TOLERANCE {
                    failures.push(format!(
                        "{name}: {:.1} nm vs Pub 151 {want} nm",
                        r.distance_nm
                    ));
                }
            }
            None => println!("{name:34} {:8.1} nm", r.distance_nm),
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
