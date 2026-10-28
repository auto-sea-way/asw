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
    /// Geofabrik PBF URL for water that is not inside the coastline: inland
    /// canals, and harbours behind a river mouth. None where the coastline
    /// already provides the water gaps.
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
        // West edge sits between the Gatun locks (-79.924) and the Rio Chagres
        // polygon below the dam (east end -79.930). The extract keeps every water
        // polygon that touches this box, and the river joins Gatun Lake at the
        // spillway, so a box that reaches it lets routes go over the dam.
        corridor: (-79.928, 8.88, -79.50, 9.42),
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
    Passage {
        name: "Vistula Spit Canal",
        corridor: (19.30, 54.348, 19.325, 54.372),
        leaf_resolution: 13, // ~25m lock chamber
        geofabrik_url: None,
        cut: Some((19.305, 54.355, 19.320, 54.358)),
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
        name: "Poros Strait",
        corridor: (23.448, 37.490, 23.466, 37.502),
        leaf_resolution: 11, // ~130m east of Poros town
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "East River",
        corridor: (-73.97, 40.745, -73.93, 40.785),
        leaf_resolution: 11, // both channels around Roosevelt Island, ~200m
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
    // Harbour behind a river mouth: the water comes from the OSM extract
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
    // Narrow sounds found by the ferry-route audit: OSM route=ferry lines whose
    // route was much longer than the ferry line. One box per blocked stretch;
    // the comment is the narrowest width measured land to land along the line.
    Passage {
        name: "Kinn, Norway 1",
        corridor: (5.1126, 61.8936, 5.1255, 61.9039),
        leaf_resolution: 12, // ~97m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Måløy, Norway",
        corridor: (5.1200, 61.9276, 5.1244, 61.9310),
        leaf_resolution: 11, // ~134m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Österåker, Sweden 1",
        corridor: (18.5774, 59.4283, 18.6016, 59.4566),
        leaf_resolution: 13, // ~21m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Österåker, Sweden 2",
        corridor: (18.6312, 59.4649, 18.6343, 59.4676),
        leaf_resolution: 11, // ~208m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Värmdö, Sweden 1",
        corridor: (18.6349, 59.4244, 18.6415, 59.4287),
        leaf_resolution: 11, // ~190m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Värmdö, Sweden 2",
        corridor: (18.5478, 59.4198, 18.5551, 59.4224),
        leaf_resolution: 13, // ~34m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Värmdö, Sweden 3",
        corridor: (18.4813, 59.4430, 18.4840, 59.4513),
        leaf_resolution: 12, // ~97m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Storäng, Sweden 1",
        corridor: (18.9250, 59.6449, 18.9283, 59.6474),
        leaf_resolution: 11, // ~174m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Storäng, Sweden 2",
        corridor: (18.9188, 59.6354, 18.9219, 59.6380),
        leaf_resolution: 11, // ~192m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Gothenburg southern archipelago, Sweden 1",
        corridor: (11.7874, 57.6151, 11.7996, 57.6213),
        leaf_resolution: 12, // ~48m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Gothenburg southern archipelago, Sweden 2",
        corridor: (11.7796, 57.5876, 11.7989, 57.6090),
        leaf_resolution: 12, // ~76m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Styrsö, Sweden",
        corridor: (11.7689, 57.6205, 11.7718, 57.6229),
        leaf_resolution: 11, // ~266m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Kragerø, Norway",
        corridor: (9.4497, 58.8626, 9.5071, 58.8857),
        leaf_resolution: 12, // ~54m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Ričina, Croatia",
        corridor: (15.2528, 44.0918, 15.2588, 44.0960),
        leaf_resolution: 11, // ~118m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Ždrelašćica, Croatia",
        corridor: (15.2497, 44.0116, 15.2628, 44.0221),
        leaf_resolution: 12, // ~54m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Muće, Croatia",
        corridor: (15.1455, 44.0232, 15.1494, 44.0257),
        leaf_resolution: 11, // ~149m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Zadar Poluotok, Croatia",
        corridor: (15.2201, 44.1161, 15.2279, 44.1201),
        leaf_resolution: 12, // ~72m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Zadar archipelago, Croatia 1",
        corridor: (15.1432, 43.9483, 15.1486, 43.9507),
        leaf_resolution: 11, // ~167m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Zadar archipelago, Croatia 2",
        corridor: (15.1663, 43.9338, 15.1717, 43.9380),
        leaf_resolution: 12, // ~77m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Djurgården, Sweden 1",
        corridor: (18.7266, 59.4543, 18.7520, 59.4667),
        leaf_resolution: 13, // ~35m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Österåker, Sweden 3",
        corridor: (18.7889, 59.4930, 18.8022, 59.4952),
        leaf_resolution: 11, // ~246m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Österåker, Sweden 4",
        corridor: (18.7726, 59.4905, 18.7750, 59.4932),
        leaf_resolution: 11, // ~188m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Djurgården, Sweden 2",
        corridor: (18.7481, 59.4761, 18.7504, 59.4791),
        leaf_resolution: 11, // ~138m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Värmdö, Sweden 4",
        corridor: (18.7659, 59.3835, 18.7743, 59.3919),
        leaf_resolution: 12, // ~42m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Gomalandet, Norway",
        corridor: (7.7326, 63.1107, 7.7567, 63.1165),
        leaf_resolution: 13, // ~40m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Houtskär, Finland 1",
        corridor: (21.1628, 60.3547, 21.1806, 60.3615),
        leaf_resolution: 11, // ~139m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Houtskär, Finland 2",
        corridor: (21.1561, 60.3288, 21.1637, 60.3422),
        leaf_resolution: 13, // ~3m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Houtskär, Finland 3",
        corridor: (21.1797, 60.3246, 21.1828, 60.3270),
        leaf_resolution: 11, // ~207m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Houtskär, Finland 4",
        corridor: (21.1851, 60.3764, 21.1919, 60.3798),
        leaf_resolution: 11, // ~226m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Houtskär, Finland 5",
        corridor: (21.1899, 60.3136, 21.1983, 60.3168),
        leaf_resolution: 13, // ~30m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Houtskär, Finland 6",
        corridor: (21.2120, 60.3091, 21.2202, 60.3129),
        leaf_resolution: 11, // ~147m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Pyin Gyi, Myanmar",
        corridor: (98.5300, 13.1665, 98.5358, 13.1772),
        leaf_resolution: 11, // ~179m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Min Ka Kye, Myanmar",
        corridor: (98.5744, 13.2463, 98.5765, 13.2495),
        leaf_resolution: 11, // ~238m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Kyat Kone, Myanmar",
        corridor: (98.5623, 13.1890, 98.5729, 13.1943),
        leaf_resolution: 11, // ~208m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Österåker, Sweden 5",
        corridor: (18.8257, 59.4760, 18.8310, 59.4838),
        leaf_resolution: 11, // ~104m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Djurgården, Sweden 3",
        corridor: (18.7500, 59.4626, 18.7629, 59.4664),
        leaf_resolution: 11, // ~127m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Hardbakke, Norway",
        corridor: (4.8204, 61.0474, 4.8226, 61.0498),
        leaf_resolution: 11, // ~203m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Steinsund, Norway",
        corridor: (4.8176, 61.0551, 4.8378, 61.0752),
        leaf_resolution: 13, // ~27m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Solund, Norway",
        corridor: (4.7902, 61.1090, 4.8055, 61.1130),
        leaf_resolution: 11, // ~194m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Buvika, Norway",
        corridor: (9.2349, 58.7193, 9.2629, 58.7314),
        leaf_resolution: 12, // ~57m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Øysang, Norway",
        corridor: (9.2541, 58.7468, 9.2604, 58.7522),
        leaf_resolution: 13, // ~26m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Bodø, Norway",
        corridor: (13.8999, 67.4055, 13.9825, 67.4302),
        leaf_resolution: 12, // ~78m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Kinn, Norway 2",
        corridor: (4.8143, 61.6057, 4.8569, 61.6203),
        leaf_resolution: 12, // ~54m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Skorpa, Norway",
        corridor: (4.7826, 61.5860, 4.7850, 61.5895),
        leaf_resolution: 11, // ~213m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Soukka, Finland",
        corridor: (24.6659, 60.1176, 24.6701, 60.1203),
        leaf_resolution: 12, // ~82m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Suvisaaristo, Finland",
        corridor: (24.6835, 60.1144, 24.6974, 60.1290),
        leaf_resolution: 11, // ~175m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Kamijima, Japan",
        corridor: (133.1694, 34.2808, 133.1782, 34.2937),
        leaf_resolution: 11, // ~122m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Ōsakikamijima, Japan",
        corridor: (133.0823, 34.3039, 133.0882, 34.3127),
        leaf_resolution: 11, // ~140m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Sandhamn, Sweden",
        corridor: (18.9112, 59.2886, 18.9226, 59.2929),
        leaf_resolution: 12, // ~51m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Rindö, Sweden",
        corridor: (18.3546, 59.3831, 18.4001, 59.4036),
        leaf_resolution: 11, // ~229m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Kure, Japan",
        corridor: (132.8533, 34.1876, 132.8654, 34.1961),
        leaf_resolution: 11, // ~142m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Storhaug, Norway",
        corridor: (5.7392, 58.9713, 5.7498, 58.9755),
        leaf_resolution: 11, // ~129m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Langøy, Norway",
        corridor: (5.7715, 59.0019, 5.7844, 59.0047),
        leaf_resolution: 12, // ~86m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Rönnäng, Sweden",
        corridor: (11.5706, 57.9231, 11.6030, 57.9370),
        leaf_resolution: 13, // ~5m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Grötö, Sweden 1",
        corridor: (11.6855, 57.6940, 11.6889, 57.6961),
        leaf_resolution: 12, // ~50m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Grötö, Sweden 2",
        corridor: (11.6832, 57.7069, 11.6886, 57.7126),
        leaf_resolution: 11, // ~135m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Hjuvik, Sweden",
        corridor: (11.6995, 57.7068, 11.7041, 57.7095),
        leaf_resolution: 13, // ~10m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Grötö, Sweden 3",
        corridor: (11.6659, 57.6976, 11.6698, 57.7001),
        leaf_resolution: 12, // ~71m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Malkenes, Norway",
        corridor: (5.6861, 60.0756, 5.6889, 60.0778),
        leaf_resolution: 13, // ~9m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Bjørnafjorden, Norway",
        corridor: (5.7044, 60.0473, 5.7072, 60.0533),
        leaf_resolution: 11, // ~155m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Tysnes, Norway",
        corridor: (5.7098, 60.0326, 5.7122, 60.0353),
        leaf_resolution: 11, // ~255m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Nes, Norway",
        corridor: (6.0039, 59.9849, 6.0076, 59.9878),
        leaf_resolution: 11, // ~220m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Laajasalo, Finland",
        corridor: (25.0264, 60.1782, 25.0687, 60.1893),
        leaf_resolution: 14, // ~10m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Fossá, Faroe Islands",
        corridor: (-6.5243, 62.2921, -6.5201, 62.2989),
        leaf_resolution: 11, // ~149m
        geofabrik_url: None,
        cut: None,
    },
    Passage {
        name: "Drivenes, Norway",
        corridor: (5.2002, 60.0068, 5.2077, 60.0163),
        leaf_resolution: 13, // ~27m
        geofabrik_url: None,
        cut: None,
    },
];
