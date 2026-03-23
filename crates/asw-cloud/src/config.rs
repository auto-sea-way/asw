/// Hetzner server configuration.
pub const HETZNER_SERVER_TYPE: &str = "ccx53";
pub const HETZNER_IMAGE: &str = "ubuntu-24.04";
pub const HETZNER_LOCATION: &str = "nbg1";
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
pub const BOOTSTRAP_PACKAGES: &str = "wget unzip curl build-essential pkg-config libssl-dev osmium-tool";

/// Bounding box presets: (min_lon, min_lat, max_lon, max_lat).
pub const DEV_BBOX: (f64, f64, f64, f64) = (-5.0, 48.0, 10.0, 62.0);
pub const DEV_BBOX_SMALL: (f64, f64, f64, f64) = (-1.0, 50.0, 3.0, 52.0);
pub const DEV_BBOX_MARMARIS: (f64, f64, f64, f64) = (27.5, 36.0, 30.0, 37.0);

/// Resolve a bbox preset name to coordinates.
pub fn resolve_bbox(name: &str) -> Option<(f64, f64, f64, f64)> {
    match name {
        "dev" => Some(DEV_BBOX),
        "dev-small" => Some(DEV_BBOX_SMALL),
        "marmaris" => Some(DEV_BBOX_MARMARIS),
        _ => None,
    }
}
