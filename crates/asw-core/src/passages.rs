/// A critical maritime passage defined by a corridor bounding box.
///
/// The system uses zone cells at `ZONE_RESOLUTION` to identify which areas
/// of the main cascade should be refined further to `leaf_resolution`.
/// This extends the adaptive cascade into narrow waterways without
/// generating flat-resolution corridor cells.
pub struct Passage {
    pub name: &'static str,
    /// Bounding box around the waterway: (min_lon, min_lat, max_lon, max_lat)
    pub corridor: (f64, f64, f64, f64),
    /// Cascade refines to this resolution within zone
    pub leaf_resolution: u8,
    /// Geofabrik PBF URL for inland canal water extraction.
    /// None for natural straits where coastline already provides water gaps.
    pub geofabrik_url: Option<&'static str>,
    /// Box across the middle of a man-made canal, closed when a request sets
    /// `canals=false`: (min_lon, min_lat, max_lon, max_lat). It cuts the
    /// transit and leaves the ports at both ends reachable. None for natural
    /// straits and for dead-end channels.
    pub cut: Option<(f64, f64, f64, f64)>,
}

/// H3 resolution used for passage-zone membership lookups.
pub const ZONE_RESOLUTION: u8 = 5;

/// Critical passages with corridor bounding boxes.
///
/// Leaf resolution guidelines by canal width:
/// - ~200m+ (Suez): res-11 (25m edge)
/// - ~33m (Panama locks): res-13 (3.5m edge)
/// - ~15m locks (Kiel): res-13 (3.5m edge)
/// - ~25m (Corinth, Lefkada): res-13 (3.5m edge)
/// - ~11m (Osor): res-14 (1.3m edge)
/// - Wide straits (Bosphorus, Dover, etc.): res-10
pub static PASSAGES: &[Passage] = &[
    Passage {
        name: "Suez Canal",
        corridor: (32.20, 29.85, 32.65, 31.32),
        leaf_resolution: 11,
        geofabrik_url: None, // sea-level canal, coastline provides gaps
        cut: Some((32.20, 30.45, 32.65, 30.50)),
    },
    Passage {
        name: "Panama Canal",
        corridor: (-79.95, 8.88, -79.50, 9.42),
        leaf_resolution: 13, // bumped from 11 — lock channels need 3.5m edges
        geofabrik_url: Some("https://download.geofabrik.de/central-america/panama-latest.osm.pbf"),
        cut: Some((-79.70, 9.04, -79.60, 9.06)),
    },
    Passage {
        name: "Kiel Canal",
        corridor: (9.05, 53.85, 10.20, 54.40),
        leaf_resolution: 13, // bumped from 11 — lock entrances need 3.5m edges
        geofabrik_url: Some(
            "https://download.geofabrik.de/europe/germany/schleswig-holstein-latest.osm.pbf",
        ),
        cut: Some((9.40, 53.85, 9.45, 54.40)),
    },
    Passage {
        name: "Corinth Canal",
        corridor: (22.94, 37.88, 23.03, 37.96),
        leaf_resolution: 13,
        geofabrik_url: None, // sea-level canal, coastline provides gaps
        cut: Some((22.980, 37.928, 22.988, 37.942)),
    },
    Passage {
        name: "Lefkada Canal",
        corridor: (20.69, 38.775, 20.75, 38.855),
        leaf_resolution: 13, // ~25m at the floating bridge
        geofabrik_url: None, // sea-level channel, coastline provides gaps
        cut: Some((20.70, 38.805, 20.75, 38.815)),
    },
    Passage {
        name: "Potidea Canal",
        corridor: (23.31, 40.185, 23.345, 40.20),
        leaf_resolution: 12, // ~40m
        geofabrik_url: None,
        cut: Some((23.327, 40.19, 23.331, 40.20)),
    },
    Passage {
        name: "Osor Channel",
        corridor: (14.38, 44.685, 14.405, 44.70),
        leaf_resolution: 14, // ~11m at the swing bridge, res-13 cells do not fit
        geofabrik_url: None,
        cut: Some((14.3917, 44.6925, 14.3924, 44.6933)),
    },
    Passage {
        name: "Privlaka Channel",
        corridor: (14.455, 44.541, 14.468, 44.549),
        leaf_resolution: 14, // narrow cut at Mali Losinj, res-13 cells do not fit
        geofabrik_url: None,
        cut: Some((14.4603, 44.5448, 14.4608, 44.5462)),
    },
    // Natural straits — coastline already provides the water gaps
    Passage {
        name: "Evripos Strait",
        corridor: (23.57, 38.44, 23.62, 38.48),
        leaf_resolution: 13, // ~40m at the Chalkis old bridge, res-12 cells do not fit
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Trogir Channel",
        corridor: (16.235, 43.51, 16.265, 43.522),
        leaf_resolution: 13, // narrow, with bridges
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Menai Strait",
        corridor: (-4.20, 53.21, -4.15, 53.23),
        leaf_resolution: 12, // the Swellies between the two bridges, rocks and islets
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Swinomish Channel",
        corridor: (-122.53, 48.36, -122.47, 48.46),
        leaf_resolution: 12, // ~100m dredged channel
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Sibenik Channel",
        corridor: (15.84, 43.71, 15.92, 43.75),
        leaf_resolution: 11, // St. Anthony Channel, ~150m; the harbour is river water in OSM
        geofabrik_url: Some("https://download.geofabrik.de/europe/croatia-latest.osm.pbf"),
        cut: None,
    },
    Passage {
        name: "Bosphorus",
        corridor: (28.95, 40.95, 29.20, 41.28),
        leaf_resolution: 10,
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Dardanelles",
        corridor: (26.10, 39.95, 26.75, 40.50),
        leaf_resolution: 10,
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Malacca Strait",
        corridor: (103.35, 1.10, 103.90, 1.40),
        leaf_resolution: 10,
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Singapore Strait",
        corridor: (103.70, 1.15, 104.35, 1.30),
        leaf_resolution: 10,
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Messina Strait",
        corridor: (15.55, 38.05, 15.70, 38.35),
        leaf_resolution: 10,
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Dover Strait",
        corridor: (1.15, 50.85, 1.70, 51.20),
        leaf_resolution: 10,
        geofabrik_url: None,
        cut: None,
    },
    // ── New canals ──────────────────────────────────────────────────────
    Passage {
        name: "Houston Ship Channel",
        corridor: (-95.30, 29.30, -94.70, 29.80),
        leaf_resolution: 12,
        geofabrik_url: Some("https://download.geofabrik.de/north-america/us/texas-latest.osm.pbf"),
        cut: None,
    },
    Passage {
        name: "Cape Cod Canal",
        corridor: (-70.65, 41.72, -70.48, 41.79),
        leaf_resolution: 12,
        geofabrik_url: Some(
            "https://download.geofabrik.de/north-america/us/massachusetts-latest.osm.pbf",
        ),
        cut: Some((-70.57, 41.75, -70.55, 41.785)),
    },
    Passage {
        name: "Cape May Canal",
        corridor: (-74.975, 38.945, -74.895, 38.972),
        leaf_resolution: 12, // ~100m
        geofabrik_url: None,
        cut: Some((-74.94, 38.945, -74.93, 38.972)),
    },
    Passage {
        name: "Chesapeake-Delaware Canal",
        corridor: (-75.85, 39.40, -75.55, 39.60),
        leaf_resolution: 12,
        geofabrik_url: Some(
            "https://download.geofabrik.de/north-america/us/delaware-latest.osm.pbf",
        ),
        cut: Some((-75.70, 39.50, -75.68, 39.58)),
    },
];
