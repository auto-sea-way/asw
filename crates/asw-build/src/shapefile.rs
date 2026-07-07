use anyhow::{Context, Result};
use asw_core::geo_index::{LandIndex, LandPolygon};
use geo::{Coord, LineString, Polygon};
use indicatif::{ProgressBar, ProgressStyle};
use shapefile::PolygonRing;
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
fn load_polygons_from_file(
    shp_path: &Path,
    bbox: Option<Bbox>,
    polygons: &mut Vec<LandPolygon>,
) -> Result<()> {
    let shapes = shapefile::read_shapes_as::<_, shapefile::Polygon>(shp_path)
        .with_context(|| format!("Failed to read shapefile {:?}", shp_path))?;
    for shp_poly in shapes {
        for poly in convert_shapefile_polygon(&shp_poly) {
            if let Some(bb) = bbox {
                if !polygon_intersects_bbox(&poly, bb) {
                    continue;
                }
            }
            polygons.push(LandPolygon::new(poly));
        }
    }
    Ok(())
}

/// Load land polygons from a shapefile or directory of shapefiles, optionally filtered by bbox.
/// Returns a LandIndex (R-tree) for point-in-water queries (inverted: not-in-land = water).
pub fn load_land_polygons(shp_path: &Path, bbox: Option<Bbox>) -> Result<LandIndex> {
    let mut polygons = Vec::new();

    if shp_path.is_dir() {
        let shp_files = find_shp_files(shp_path)?;
        info!(
            "Loading land polygons from {} shapefiles in {:?}",
            shp_files.len(),
            shp_path
        );
        let pb = ProgressBar::new(shp_files.len() as u64);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("[{elapsed_precise}] {bar:40} {pos}/{len} shapefiles")
                .unwrap(),
        );
        for f in &shp_files {
            load_polygons_from_file(f, bbox, &mut polygons)?;
            pb.inc(1);
        }
        pb.finish_with_message("done");
    } else {
        info!("Loading land polygons from {:?}", shp_path);
        let shapes = shapefile::read_shapes_as::<_, shapefile::Polygon>(shp_path)
            .context("Failed to read shapefile")?;
        let pb = ProgressBar::new(shapes.len() as u64);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("[{elapsed_precise}] {bar:40} {pos}/{len} polygons")
                .unwrap(),
        );
        for shp_poly in shapes {
            pb.inc(1);
            for poly in convert_shapefile_polygon(&shp_poly) {
                if let Some(bb) = bbox {
                    if !polygon_intersects_bbox(&poly, bb) {
                        continue;
                    }
                }
                polygons.push(LandPolygon::new(poly));
            }
        }
        pb.finish_with_message("done");
    }

    info!("Loaded {} land polygons", polygons.len());
    Ok(LandIndex::new(polygons))
}

/// Load raw geo::Polygons from a shapefile or directory of shapefiles (for coastline extraction).
pub fn load_raw_polygons(shp_path: &Path, bbox: Option<Bbox>) -> Result<Vec<Polygon<f64>>> {
    let mut polygons = Vec::new();

    if shp_path.is_dir() {
        let shp_files = find_shp_files(shp_path)?;
        for f in &shp_files {
            load_raw_from_file(f, bbox, &mut polygons)?;
        }
    } else {
        load_raw_from_file(shp_path, bbox, &mut polygons)?;
    }

    Ok(polygons)
}

fn load_raw_from_file(
    shp_path: &Path,
    bbox: Option<Bbox>,
    polygons: &mut Vec<Polygon<f64>>,
) -> Result<()> {
    let shapes = shapefile::read_shapes_as::<_, shapefile::Polygon>(shp_path)
        .with_context(|| format!("Failed to read shapefile {:?}", shp_path))?;
    for shp_poly in shapes {
        for poly in convert_shapefile_polygon(&shp_poly) {
            if let Some(bb) = bbox {
                if !polygon_intersects_bbox(&poly, bb) {
                    continue;
                }
            }
            polygons.push(poly);
        }
    }
    Ok(())
}

/// Convert a shapefile polygon into geo::Polygons.
/// Groups rings: each Outer ring starts a new polygon, Inner rings are holes.
fn convert_shapefile_polygon(shp_poly: &shapefile::Polygon) -> Vec<Polygon<f64>> {
    let rings = shp_poly.rings();
    let mut result = Vec::new();
    let mut current_exterior: Option<LineString<f64>> = None;
    let mut current_holes: Vec<LineString<f64>> = Vec::new();

    for ring in rings {
        let (points, is_outer) = match ring {
            PolygonRing::Outer(pts) => (pts, true),
            PolygonRing::Inner(pts) => (pts, false),
        };

        let coords: Vec<Coord<f64>> = points.iter().map(|p| Coord { x: p.x, y: p.y }).collect();
        let ls = LineString::new(coords);

        if is_outer {
            // Flush previous polygon
            if let Some(ext) = current_exterior.take() {
                result.push(Polygon::new(ext, std::mem::take(&mut current_holes)));
            }
            current_exterior = Some(ls);
            current_holes.clear();
        } else {
            current_holes.push(ls);
        }
    }

    // Flush last polygon
    if let Some(ext) = current_exterior {
        result.push(Polygon::new(ext, current_holes));
    }

    result
}

pub fn polygon_intersects_bbox(poly: &Polygon<f64>, bbox: Bbox) -> bool {
    let (min_lon, min_lat, max_lon, max_lat) = bbox;
    let mut p_min_x = f64::MAX;
    let mut p_min_y = f64::MAX;
    let mut p_max_x = f64::MIN;
    let mut p_max_y = f64::MIN;
    for coord in poly.exterior().coords() {
        p_min_x = p_min_x.min(coord.x);
        p_min_y = p_min_y.min(coord.y);
        p_max_x = p_max_x.max(coord.x);
        p_max_y = p_max_y.max(coord.y);
    }
    !(p_max_x < min_lon || p_min_x > max_lon || p_max_y < min_lat || p_min_y > max_lat)
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

/// Extract the flat (non-directory) files of a zip archive into `extract_dir`, atomically.
///
/// Entries are written into a temporary sibling directory first; `extract_dir` itself is
/// only populated (via `fs::rename`) once every entry has been extracted without error. This
/// means a failure partway through extraction (corrupt zip entry, truncated/interrupted
/// download, disk full) never leaves a partial `extract_dir` behind for a later run's cache
/// check (`find_shp_files(&extract_dir)` in `download_and_extract`) to mistake for a
/// complete, valid extraction. Mirrors the `.pbf.tmp` download-then-rename pattern already
/// used in `canal_water.rs`.
fn extract_zip_atomic<R: Read + Seek>(reader: R, extract_dir: &Path) -> Result<()> {
    let mut archive = zip::ZipArchive::new(reader).context("Failed to read zip")?;

    let tmp_name = format!(
        "{}.extracting.tmp",
        extract_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "extract".to_string())
    );
    let tmp_dir = extract_dir.with_file_name(tmp_name);
    // Clean up any leftovers from a previously interrupted extraction attempt.
    let _ = std::fs::remove_dir_all(&tmp_dir);
    std::fs::create_dir_all(&tmp_dir).context("Failed to create temporary extraction dir")?;

    let extracted: Result<()> = (|| {
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i)?;
            let name = entry.name().to_string();
            if let Some(filename) = name.rsplit('/').next() {
                if !filename.is_empty() {
                    let out_path = tmp_dir.join(filename);
                    let mut out_file = std::fs::File::create(&out_path)
                        .with_context(|| format!("Failed to create {:?}", out_path))?;
                    std::io::copy(&mut entry, &mut out_file)
                        .with_context(|| format!("Failed to extract entry {:?}", name))?;
                }
            }
        }
        Ok(())
    })();

    if let Err(e) = extracted {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(e);
    }

    // extract_dir should not exist yet (the caller only reaches here when the cache check
    // failed), but guard against a stale partial directory from an older, pre-atomic run.
    let _ = std::fs::remove_dir_all(extract_dir);
    std::fs::rename(&tmp_dir, extract_dir).context("Failed to move extracted files into place")?;
    Ok(())
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
