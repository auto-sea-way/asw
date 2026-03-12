/// A critical maritime passage defined by a corridor bounding box.
///
/// The system uses zone cells at `zone_resolution` to identify which areas
/// of the main cascade should be refined further to `leaf_resolution`.
/// This extends the adaptive cascade into narrow waterways without
/// generating flat-resolution corridor cells.
pub struct Passage {
    pub name: &'static str,
    /// Bounding box around the waterway: (min_lon, min_lat, max_lon, max_lat)
    pub corridor: (f64, f64, f64, f64),
    /// H3 resolution for zone membership (typically 5)
    pub zone_resolution: u8,
    /// Cascade refines to this resolution within zone
    pub leaf_resolution: u8,
}

/// Critical passages with corridor bounding boxes.
///
/// Leaf resolution guidelines by canal width:
/// - ~200m+ (Suez, Panama): res-11 (25m edge)
/// - ~100m (Kiel): res-11 (25m edge)
/// - ~25m (Corinth): res-13 (3.5m edge)
/// - Wide straits (Bosphorus, Dover, etc.): res-10
pub static PASSAGES: &[Passage] = &[
    Passage {
        name: "Suez Canal",
        corridor: (32.20, 29.85, 32.65, 31.32),
        zone_resolution: 5,
        leaf_resolution: 11,
    },
    Passage {
        name: "Panama Canal",
        corridor: (-79.95, 8.88, -79.50, 9.42),
        zone_resolution: 5,
        leaf_resolution: 11,
    },
    Passage {
        name: "Kiel Canal",
        corridor: (9.05, 53.85, 10.20, 54.40),
        zone_resolution: 5,
        leaf_resolution: 11,
    },
    Passage {
        name: "Corinth Canal",
        corridor: (22.94, 37.88, 23.03, 37.96),
        zone_resolution: 5,
        leaf_resolution: 13,
    },
    Passage {
        name: "Bosphorus",
        corridor: (28.95, 40.95, 29.20, 41.28),
        zone_resolution: 5,
        leaf_resolution: 10,
    },
    Passage {
        name: "Dardanelles",
        corridor: (26.10, 39.95, 26.75, 40.50),
        zone_resolution: 5,
        leaf_resolution: 10,
    },
    Passage {
        name: "Malacca Strait",
        corridor: (103.35, 1.10, 103.90, 1.40),
        zone_resolution: 5,
        leaf_resolution: 10,
    },
    Passage {
        name: "Singapore Strait",
        corridor: (103.70, 1.15, 104.35, 1.30),
        zone_resolution: 5,
        leaf_resolution: 10,
    },
    Passage {
        name: "Messina Strait",
        corridor: (15.55, 38.05, 15.70, 38.35),
        zone_resolution: 5,
        leaf_resolution: 10,
    },
    Passage {
        name: "Dover Strait",
        corridor: (1.15, 50.85, 1.70, 51.20),
        zone_resolution: 5,
        leaf_resolution: 10,
    },
];
