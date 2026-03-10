/// A critical maritime passage defined by a sequence of (lat, lon) waypoints.
pub struct Passage {
    pub name: &'static str,
    pub waypoints: &'static [(f64, f64)],
}

/// All 10 critical passages.
pub static PASSAGES: &[Passage] = &[
    Passage {
        name: "Suez Canal",
        waypoints: &[
            (29.9167, 32.5500),
            (30.2000, 32.3400),
            (30.4500, 32.3500),
            (30.7167, 32.3400),
            (31.0000, 32.3200),
            (31.2667, 32.3100),
        ],
    },
    Passage {
        name: "Panama Canal",
        waypoints: &[
            (8.9500, -79.5700),
            (9.1000, -79.7000),
            (9.2800, -79.9200),
        ],
    },
    Passage {
        name: "Kiel Canal",
        waypoints: &[
            (54.3667, 10.1500),
            (54.3333, 9.9500),
            (54.3167, 9.7000),
            (54.3167, 9.5000),
            (54.2833, 9.1500),
        ],
    },
    Passage {
        name: "Corinth Canal",
        waypoints: &[
            (37.9333, 22.9833),
            (37.9167, 23.0000),
            (37.9000, 23.0167),
        ],
    },
    Passage {
        name: "Bosphorus",
        waypoints: &[
            (41.2167, 29.1000),
            (41.1167, 29.0667),
            (41.0667, 29.0167),
            (41.0000, 29.0000),
        ],
    },
    Passage {
        name: "Dardanelles",
        waypoints: &[
            (40.4500, 26.6667),
            (40.3500, 26.5000),
            (40.2167, 26.4000),
            (40.0500, 26.2000),
        ],
    },
    Passage {
        name: "Malacca Strait",
        waypoints: &[
            (1.2667, 103.8500),
            (1.1833, 103.7500),
            (1.1667, 103.5000),
        ],
    },
    Passage {
        name: "Singapore Strait",
        waypoints: &[
            (1.2667, 103.8500),
            (1.2333, 104.0000),
            (1.2500, 104.1500),
        ],
    },
    Passage {
        name: "Messina Strait",
        waypoints: &[
            (38.2667, 15.6333),
            (38.2000, 15.6167),
            (38.1000, 15.6500),
        ],
    },
    Passage {
        name: "Dover Strait",
        waypoints: &[
            (51.1000, 1.3500),
            (50.9667, 1.5000),
            (50.8833, 1.7000),
        ],
    },
];
