//! Download and extract canal water polygons from Geofabrik PBF files.

use anyhow::{Context, Result};
use asw_core::passages::Passage;
use geo::{Coord, LineString, Polygon};
use std::path::Path;
use std::process::Command;
use tracing::info;

use crate::shapefile::Bbox;

/// Extract canal water polygons for all passages that have a `geofabrik_url`.
pub fn extract_canal_water(
    passages: &[Passage],
    build_bbox: Option<Bbox>,
    work_dir: &Path,
) -> Result<Vec<Polygon<f64>>> {
    let canal_dir = work_dir.join("canal-water");
    std::fs::create_dir_all(&canal_dir)?;

    // Check osmium is available
    let osmium_check = Command::new("osmium").arg("--version").output();
    if osmium_check.is_err() || !osmium_check.unwrap().status.success() {
        anyhow::bail!(
            "osmium-tool is required for canal water extraction. \
             Install: apt install osmium-tool (Linux) or brew install osmium-tool (macOS)"
        );
    }

    let mut all_water = Vec::new();

    for passage in passages {
        let url = match passage.geofabrik_url {
            Some(url) => url,
            None => continue,
        };

        if passage.water_types.is_empty() {
            continue;
        }

        // Skip if passage corridor doesn't overlap build bbox
        if let Some(bb) = build_bbox {
            let (p_min_lon, p_min_lat, p_max_lon, p_max_lat) = passage.corridor;
            let (b_min_lon, b_min_lat, b_max_lon, b_max_lat) = bb;
            if p_max_lon < b_min_lon
                || p_min_lon > b_max_lon
                || p_max_lat < b_min_lat
                || p_min_lat > b_max_lat
            {
                info!("Skipping canal '{}' — outside build bbox", passage.name);
                continue;
            }
        }

        info!("Processing canal water for '{}'...", passage.name);
        let water = extract_single_passage(passage, url, &canal_dir)?;
        info!("  {} water polygons for '{}'", water.len(), passage.name);
        all_water.extend(water);
    }

    if !all_water.is_empty() {
        info!("Total canal water polygons: {}", all_water.len());
    }

    Ok(all_water)
}

fn extract_single_passage(
    passage: &Passage,
    url: &str,
    work_dir: &Path,
) -> Result<Vec<Polygon<f64>>> {
    let safe_name = passage.name.to_lowercase().replace(' ', "-");
    let pbf_path = work_dir.join(format!("{}.osm.pbf", safe_name));
    let water_pbf_path = work_dir.join(format!("{}-water.osm.pbf", safe_name));
    let geojson_path = work_dir.join(format!("{}-water.geojson", safe_name));

    // Step 1: Download PBF (with caching)
    if !pbf_path.exists() {
        info!("  Downloading {}...", url);
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(30))
            .timeout(std::time::Duration::from_secs(600))
            .build()?;
        let mut resp = client.get(url).send().context("Failed to download PBF")?;
        let mut file = std::fs::File::create(&pbf_path)?;
        let bytes = std::io::copy(&mut resp, &mut file)?;
        info!("  Downloaded {} MB", bytes / 1_000_000);
    } else {
        info!("  Using cached PBF: {:?}", pbf_path);
    }

    // Step 2: osmium tags-filter for natural=water
    if !water_pbf_path.exists() {
        info!("  Filtering for natural=water...");
        let status = Command::new("osmium")
            .args([
                "tags-filter",
                pbf_path.to_str().unwrap(),
                "nwr/natural=water",
                "-o",
                water_pbf_path.to_str().unwrap(),
                "--overwrite",
            ])
            .status()
            .context("Failed to run osmium tags-filter")?;
        if !status.success() {
            anyhow::bail!("osmium tags-filter failed for {}", passage.name);
        }
    }

    // Step 3: osmium export to GeoJSON
    info!("  Exporting to GeoJSON...");
    let status = Command::new("osmium")
        .args([
            "export",
            water_pbf_path.to_str().unwrap(),
            "-o",
            geojson_path.to_str().unwrap(),
            "--overwrite",
        ])
        .status()
        .context("Failed to run osmium export")?;
    if !status.success() {
        anyhow::bail!("osmium export failed for {}", passage.name);
    }

    // Step 4: Parse GeoJSON, clip to corridor, filter by water type
    let geojson_str = std::fs::read_to_string(&geojson_path)?;
    let geojson: geojson::GeoJson = geojson_str.parse().context("Failed to parse GeoJSON")?;

    let (min_lon, min_lat, max_lon, max_lat) = passage.corridor;
    let water_types: std::collections::HashSet<&str> =
        passage.water_types.iter().copied().collect();

    let mut polygons = Vec::new();

    if let geojson::GeoJson::FeatureCollection(fc) = geojson {
        for feature in fc.features {
            if let Some(ref props) = feature.properties {
                let water_val = props.get("water").and_then(|v| v.as_str()).unwrap_or("");
                if !water_val.is_empty() && !water_types.contains(water_val) {
                    continue;
                }
                let natural = props.get("natural").and_then(|v| v.as_str()).unwrap_or("");
                if natural != "water" {
                    continue;
                }
            } else {
                continue;
            }

            if let Some(geom) = feature.geometry {
                let polys = geojson_geometry_to_polygons(&geom.value);
                for poly in polys {
                    if polygon_intersects_bbox(&poly, (min_lon, min_lat, max_lon, max_lat)) {
                        polygons.push(poly);
                    }
                }
            }
        }
    }

    // Cleanup intermediate files (keep PBF cache)
    let _ = std::fs::remove_file(&geojson_path);
    let _ = std::fs::remove_file(&water_pbf_path);

    Ok(polygons)
}

fn geojson_geometry_to_polygons(value: &geojson::Value) -> Vec<Polygon<f64>> {
    match value {
        geojson::Value::Polygon(coords) => coords_to_polygon(coords).into_iter().collect(),
        geojson::Value::MultiPolygon(multi) => {
            multi.iter().filter_map(|c| coords_to_polygon(c)).collect()
        }
        _ => vec![],
    }
}

fn coords_to_polygon(coords: &[Vec<Vec<f64>>]) -> Option<Polygon<f64>> {
    if coords.is_empty() {
        return None;
    }
    let exterior = LineString::new(
        coords[0]
            .iter()
            .map(|c| Coord { x: c[0], y: c[1] })
            .collect(),
    );
    let holes: Vec<LineString<f64>> = coords[1..]
        .iter()
        .map(|ring| LineString::new(ring.iter().map(|c| Coord { x: c[0], y: c[1] }).collect()))
        .collect();
    Some(Polygon::new(exterior, holes))
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
