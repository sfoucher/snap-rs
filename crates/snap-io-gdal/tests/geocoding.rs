mod fixtures;
use fixtures::*;
use gdal::spatial_ref::SpatialRef;
use gdal::DriverManager;
use snap_core::{Crs, CrsGeoCoding, DataType, GeoCoding};
use snap_io::{ProductReader, Registry};
use snap_io_gdal::{register, GeoTiffReader};

#[test]
fn geotiff_product_fields() {
    let dir = temp_dir("gt-fields");
    let f = make_fixture(
        &dir,
        "a.tif",
        300,
        200,
        DataType::I16,
        3,
        false,
        Some(-9999.0),
        Some(32618),
    );
    let p = GeoTiffReader.read(&f.path).unwrap();
    assert_eq!(p.product_type, "GeoTIFF");
    assert_eq!(p.name, "a");
    assert_eq!((p.width, p.height), (300, 200));
    let names: Vec<_> = p.bands.iter().map(|b| b.name.as_str()).collect();
    assert_eq!(names, ["band_1", "band_2", "band_3"]);
    for b in &p.bands {
        assert_eq!(b.no_data, Some(-9999.0));
        assert_eq!(b.data_type, DataType::I16);
        assert_eq!((b.width, b.height), (300, 200));
    }
    assert!(p.metadata.get_path("IMAGE_STRUCTURE/INTERLEAVE").is_some());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn geocoding_matches_gdaltransform() {
    let dir = temp_dir("gt-geo");
    let f = make_fixture(
        &dir,
        "a.tif",
        300,
        200,
        DataType::U8,
        1,
        false,
        None,
        Some(32618),
    );
    let p = GeoTiffReader.read(&f.path).unwrap();
    let wkt = SpatialRef::from_epsg(32618).unwrap().to_wkt().unwrap();
    let direct = CrsGeoCoding::new(
        f.gt,
        Crs {
            wkt,
            epsg: Some(32618),
            name: String::new(),
        },
    )
    .unwrap();
    let a = p.geocoding.pixel_to_geo(100.0, 50.0).unwrap();
    let b = direct.pixel_to_geo(100.0, 50.0).unwrap();
    assert!((a.lon - b.lon).abs() < 1e-6 && (a.lat - b.lat).abs() < 1e-6);
    assert!((a.lon - (-72.4322371741319)).abs() < 1e-6);
    assert!((a.lat - 45.3901490956265).abs() < 1e-6);
    assert_eq!(p.geocoding.crs().unwrap().epsg, Some(32618));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn geotiff_without_georef_uses_nogeocoding() {
    let dir = temp_dir("gt-nogeo");
    let f = make_fixture(&dir, "a.tif", 64, 64, DataType::U8, 1, false, None, None);
    let p = GeoTiffReader.read(&f.path).unwrap();
    assert!(p.geocoding.pixel_to_geo(0.0, 0.0).is_none());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn registry_picks_geotiff_over_generic() {
    let dir = temp_dir("gt-reg");
    let f = make_fixture(&dir, "a.tif", 64, 64, DataType::U8, 1, false, None, None);
    let png = dir.join("a.png");
    let src = gdal::Dataset::open(&f.path).unwrap();
    let opts = gdal::raster::RasterCreationOptions::new();
    src.create_copy(
        &DriverManager::get_driver_by_name("PNG").unwrap(),
        &png,
        &opts,
    )
    .unwrap();
    let mut reg = Registry::new();
    register(&mut reg);
    assert_eq!(reg.open(&f.path).unwrap().product_type, "GeoTIFF");
    assert_eq!(reg.open(&png).unwrap().product_type, "GDAL:PNG");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn projection_without_geotransform_uses_nogeocoding() {
    let dir = temp_dir("gt-nogt");
    let path = dir.join("p.tif");
    let drv = DriverManager::get_driver_by_name("GTiff").unwrap();
    let mut ds = drv
        .create_with_band_type::<u8, _>(&path, 16, 16, 1)
        .unwrap();
    ds.set_projection(&SpatialRef::from_epsg(32618).unwrap().to_wkt().unwrap())
        .unwrap();
    drop(ds);
    let p = GeoTiffReader.read(&path).unwrap();
    assert!(p.geocoding.pixel_to_geo(0.0, 0.0).is_none());
    std::fs::remove_dir_all(dir).unwrap();
}
