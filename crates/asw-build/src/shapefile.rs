use anyhow::{Context, Result};
use asw_core::geo_index::{LandIndex, LandPolygon};
use geo::{BoundingRect, MultiPolygon, Polygon};
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};
use tracing::info;

/// Bounding box: (min_lon, min_lat, max_lon, max_lat)
pub type Bbox = (f64, f64, f64, f64);

/// Find all .shp files in a directory.
fn find_shp_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).context("Failed to read shapefile directory")? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().map(|e| e == "shp").unwrap_or(false) {
            files.push(path);
        }
    }
    files.sort();
    anyhow::ensure!(!files.is_empty(), "No .shp files found in {:?}", dir);
    Ok(files)
}

/// Load polygons from a single shapefile into the provided vec.
fn load_polygons_from_file(shp_path: &Path, polygons: &mut Vec<LandPolygon>) -> Result<()> {
    let shapes = shapefile::read_shapes_as::<_, shapefile::Polygon>(shp_path)
        .with_context(|| format!("Failed to read shapefile {:?}", shp_path))?;
    for shp_poly in shapes {
        let multi = MultiPolygon::try_from(shp_poly)
            .with_context(|| format!("Malformed polygon rings in {:?}", shp_path))?;
        polygons.extend(multi.0.into_iter().map(LandPolygon::new));
    }
    Ok(())
}

/// Load land polygons from a shapefile or directory of shapefiles.
/// Returns a LandIndex (R-tree) for point-in-water queries (inverted: not-in-land = water).
/// Never bbox-filtered: low-resolution H3 cells extend far beyond any build bbox,
/// and the R-tree handles spatial queries efficiently.
pub fn load_land_polygons(shp_path: &Path) -> Result<LandIndex> {
    let mut polygons = Vec::new();

    if shp_path.is_dir() {
        let shp_files = find_shp_files(shp_path)?;
        info!(
            "Loading land polygons from {} shapefiles in {:?}",
            shp_files.len(),
            shp_path
        );
        let pb = crate::cells::make_progress(shp_files.len(), "shapefiles");
        for f in &shp_files {
            load_polygons_from_file(f, &mut polygons)?;
            pb.inc(1);
        }
        pb.finish_and_clear();
    } else {
        info!("Loading land polygons from {:?}", shp_path);
        load_polygons_from_file(shp_path, &mut polygons)?;
    }

    info!("Loaded {} land polygons", polygons.len());
    Ok(LandIndex::new(polygons))
}

pub fn polygon_intersects_bbox(poly: &Polygon<f64>, bbox: Bbox) -> bool {
    let (min_lon, min_lat, max_lon, max_lat) = bbox;
    let Some(r) = poly.bounding_rect() else {
        return false;
    };
    !(r.max().x < min_lon || r.min().x > max_lon || r.max().y < min_lat || r.min().y > max_lat)
}

/// Download and extract the land polygons shapefile.
pub fn download_and_extract(output_dir: &Path) -> Result<PathBuf> {
    let url = "https://osmdata.openstreetmap.de/download/land-polygons-split-4326.zip";
    let zip_path = output_dir.join("land-polygons-split-4326.zip");
    let extract_dir = output_dir.join("land-polygons-split-4326");

    if extract_dir.is_dir()
        && find_shp_files(&extract_dir)
            .map(|f| !f.is_empty())
            .unwrap_or(false)
    {
        info!("Shapefiles already exist at {:?}", extract_dir);
        return Ok(extract_dir);
    }

    info!("Downloading land polygons from {}", url);
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(30))
        .timeout(std::time::Duration::from_secs(1800)) // 30 min for ~900 MB
        .build()
        .context("Failed to build HTTP client")?;

    let mut resp = client
        .get(url)
        .send()
        .context("Failed to download shapefile")?
        .error_for_status()
        .context("Shapefile download returned a non-success HTTP status")?;
    let mut out_file = std::fs::File::create(&zip_path).context("Failed to create zip file")?;
    let bytes_copied = std::io::copy(&mut resp, &mut out_file).context("Failed to write zip")?;
    info!("Downloaded {} MB", bytes_copied / 1_000_000);

    info!("Extracting...");
    let file = std::fs::File::open(&zip_path).context("Failed to open zip")?;
    extract_zip_atomic(file, &extract_dir).context("Failed to extract shapefile zip")?;

    let _ = std::fs::remove_file(&zip_path);
    info!("Extracted shapefiles to {:?}", extract_dir);
    Ok(extract_dir)
}

/// Extract a zip archive into `extract_dir` (dropping its single root directory,
/// if any), atomically: entries go to a temporary sibling directory first, which
/// is renamed into place only once every entry extracted cleanly. A corrupt entry
/// or interrupted download therefore never leaves a partial `extract_dir` for a
/// later run's cache check to mistake for a complete extraction.
fn extract_zip_atomic<R: Read + Seek>(reader: R, extract_dir: &Path) -> Result<()> {
    let mut archive = zip::ZipArchive::new(reader).context("Failed to read zip")?;
    let tmp_dir = extract_dir.with_extension("extracting.tmp");
    let _ = std::fs::remove_dir_all(&tmp_dir);
    if let Err(e) = archive.extract_unwrapped_root_dir(&tmp_dir, zip::read::root_dir_common_filter)
    {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(e).context("Failed to extract zip");
    }
    // Guard against a stale partial directory from an older, pre-atomic run.
    let _ = std::fs::remove_dir_all(extract_dir);
    std::fs::rename(&tmp_dir, extract_dir).context("Failed to move extracted files into place")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    use std::sync::atomic::{AtomicU64, Ordering};
    use zip::write::SimpleFileOptions;

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    /// A directory path under the system temp dir, unique per call (nanosecond timestamp +
    /// atomic counter), so parallel tests never collide.
    fn unique_temp_dir(label: &str) -> PathBuf {
        let n = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("asw_build_shapefile_test_{label}_{nanos}_{n}"))
    }

    fn build_test_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buf);
            let options =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
            for (name, data) in entries {
                writer.start_file(*name, options).unwrap();
                writer.write_all(data).unwrap();
            }
            writer.finish().unwrap();
        }
        buf.into_inner()
    }

    #[test]
    fn extract_zip_atomic_extracts_valid_entries() {
        let zip_bytes = build_test_zip(&[
            ("land-polygons/a.shp", b"AAAA-shape-data"),
            ("land-polygons/b.dbf", b"BBBB-attribute-data"),
        ]);
        let extract_dir = unique_temp_dir("valid");
        let _ = std::fs::remove_dir_all(&extract_dir);

        let result = extract_zip_atomic(Cursor::new(zip_bytes), &extract_dir);
        assert!(result.is_ok(), "expected success, got {:?}", result.err());
        assert!(extract_dir.join("a.shp").is_file());
        assert!(extract_dir.join("b.dbf").is_file());
        assert_eq!(
            std::fs::read(extract_dir.join("a.shp")).unwrap(),
            b"AAAA-shape-data"
        );

        let _ = std::fs::remove_dir_all(&extract_dir);
    }

    /// Covers the extraction-atomicity half of finding 10: a zip that opens fine but has a
    /// corrupted entry partway through must not leave a usable `extract_dir` behind for a
    /// later run's cache check (`download_and_extract`'s `find_shp_files` probe) to mistake
    /// for a complete, valid extraction. The HTTP-status half (`.error_for_status()`) is not
    /// covered here since it requires a live/mocked network response; that call is a single
    /// line reviewed by hand (see `download_and_extract`).
    #[test]
    fn extract_zip_atomic_leaves_no_extract_dir_on_corrupt_entry() {
        let mut zip_bytes = build_test_zip(&[
            ("a.shp", b"good-data-one"),
            ("b.dbf", b"CORRUPT-ME-PAYLOAD"),
        ]);

        // Flip a byte inside the second entry's raw (stored, uncompressed) payload so its
        // CRC32 check fails on read, without touching the archive's local/central headers —
        // this simulates a truncated/interrupted download or a bad zip entry, distinct from
        // an outright unparsable archive.
        let marker: &[u8] = b"CORRUPT-ME-PAYLOAD";
        let pos = zip_bytes
            .windows(marker.len())
            .position(|w| w == marker)
            .expect("marker bytes not found in zip data");
        zip_bytes[pos] ^= 0xFF;

        let extract_dir = unique_temp_dir("corrupt");
        let _ = std::fs::remove_dir_all(&extract_dir);

        let result = extract_zip_atomic(Cursor::new(zip_bytes), &extract_dir);
        assert!(result.is_err(), "expected corrupt entry to fail extraction");
        assert!(
            !extract_dir.exists(),
            "a failed extraction must not leave a usable extract_dir behind (a later run's \
             cache check would mistake it for a complete extraction)"
        );

        // No temp directory should be left behind either.
        let tmp_name = format!(
            "{}.extracting.tmp",
            extract_dir.file_name().unwrap().to_string_lossy()
        );
        assert!(!extract_dir.with_file_name(tmp_name).exists());
    }
}
