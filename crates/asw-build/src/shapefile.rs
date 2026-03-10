use anyhow::{Context, Result};
use geo::{Coord, LineString, Polygon};
use indicatif::{ProgressBar, ProgressStyle};
use shapefile::PolygonRing;
use asw_core::geo_index::{LandIndex, LandPolygon};
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
        info!("Loading land polygons from {} shapefiles in {:?}", shp_files.len(), shp_path);
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

        let coords: Vec<Coord<f64>> = points
            .iter()
            .map(|p| Coord { x: p.x, y: p.y })
            .collect();
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

fn polygon_intersects_bbox(poly: &Polygon<f64>, bbox: Bbox) -> bool {
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

    if extract_dir.is_dir() && find_shp_files(&extract_dir).map(|f| !f.is_empty()).unwrap_or(false) {
        info!("Shapefiles already exist at {:?}", extract_dir);
        return Ok(extract_dir);
    }

    info!("Downloading land polygons from {}", url);
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(30))
        .timeout(std::time::Duration::from_secs(1800)) // 30 min for ~900 MB
        .build()
        .context("Failed to build HTTP client")?;

    let mut resp = client.get(url).send().context("Failed to download shapefile")?;
    let mut out_file = std::fs::File::create(&zip_path).context("Failed to create zip file")?;
    let bytes_copied = std::io::copy(&mut resp, &mut out_file).context("Failed to write zip")?;
    info!("Downloaded {} MB", bytes_copied / 1_000_000);

    info!("Extracting...");
    let file = std::fs::File::open(&zip_path).context("Failed to open zip")?;
    let mut archive = zip::ZipArchive::new(file).context("Failed to read zip")?;

    std::fs::create_dir_all(&extract_dir)?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let name = entry.name().to_string();
        if let Some(filename) = name.rsplit('/').next() {
            if !filename.is_empty() {
                let out_path = extract_dir.join(filename);
                let mut out_file = std::fs::File::create(&out_path)?;
                std::io::copy(&mut entry, &mut out_file)?;
            }
        }
    }

    let _ = std::fs::remove_file(&zip_path);
    info!("Extracted shapefiles to {:?}", extract_dir);
    Ok(extract_dir)
}
