/// Hetzner server configuration.
pub const HETZNER_SERVER_TYPE: &str = "ccx53";
pub const HETZNER_IMAGE: &str = "ubuntu-24.04";
pub const HETZNER_SERVER_NAME: &str = "asw-builder";

/// Location fallback order.
pub const HETZNER_LOCATIONS: &[&str] = &["nbg1", "fsn1", "hel1"];

/// Remote paths.
pub const REMOTE_SRC_DIR: &str = "/opt/asw";
pub const REMOTE_DATA_DIR: &str = "/data/asw";
pub const REMOTE_BIN: &str = "/usr/local/bin/asw";

/// Land polygons download URL.
pub const LAND_POLYGONS_URL: &str =
    "https://osmdata.openstreetmap.de/download/land-polygons-split-4326.zip";

/// Packages to install during bootstrap.
pub const BOOTSTRAP_PACKAGES: &str =
    "wget unzip curl build-essential pkg-config libssl-dev osmium-tool";

/// Resolve a bbox preset name to (min_lon, min_lat, max_lon, max_lat).
pub fn resolve_bbox(name: &str) -> Option<(f64, f64, f64, f64)> {
    match name {
        "dev" => Some((-5.0, 48.0, 10.0, 62.0)),
        "dev-small" => Some((-1.0, 50.0, 3.0, 52.0)),
        "marmaris" => Some((27.5, 36.0, 30.0, 37.0)),
        _ => None,
    }
}
