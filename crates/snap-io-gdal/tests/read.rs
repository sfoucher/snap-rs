mod fixtures;
use fixtures::*;
use snap_core::{level_size, DataType, RasterSource};
use snap_io::IoError;
use snap_io_gdal::GdalSource;

#[test]
fn tile_matches_gdal_read() {
    let dir = temp_dir("read");
    let f = make_fixture(&dir, "a.tif", 1000, 700, DataType::U16, 1, true, None, None);
    let s = GdalSource::open(&f.path, 1).unwrap();
    let t = s.read_tile(0, 1, 1).unwrap();
    assert_eq!(t.shape(), (188, 488));
    assert_eq!(t.get_f64(0, 0), ((512 * 1000 + 512) % 1000) as f64);
    assert_eq!(t.get_f64(0, 1), 513.0);
    assert!(s.read_tile(0, 2, 0).is_err());
    // width 1001: value = (row + col) % 1000, so a wrong y offset or transpose fails
    let g = make_fixture(
        &dir,
        "g.tif",
        1001,
        700,
        DataType::U16,
        2,
        false,
        None,
        None,
    );
    let t = GdalSource::open(&g.path, 1)
        .unwrap()
        .read_tile(0, 0, 1)
        .unwrap();
    assert_eq!(t.shape(), (188, 512));
    assert_eq!(
        (t.get_f64(0, 0), t.get_f64(1, 0), t.get_f64(0, 1)),
        (512.0, 513.0, 513.0)
    );
    let t2 = GdalSource::open(&g.path, 2)
        .unwrap()
        .read_tile(0, 0, 1)
        .unwrap();
    assert_eq!(t2.get_f64(0, 0), 612.0);
    drop(s);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn overview_levels_reported() {
    let dir = temp_dir("ovr");
    let a = make_fixture(&dir, "a.tif", 1000, 700, DataType::U16, 1, true, None, None);
    let b = make_fixture(
        &dir,
        "b.tif",
        1000,
        700,
        DataType::U16,
        1,
        false,
        None,
        None,
    );
    let sa = GdalSource::open(&a.path, 1).unwrap();
    assert_eq!(sa.levels(), 2);
    assert_eq!(GdalSource::open(&b.path, 1).unwrap().levels(), 1);
    // the pyramid assumes GDAL overview sizes follow level_size (ceil)
    let (w, h) = level_size(1000, 700, 1);
    assert_eq!((w, h), (500, 350));
    let t = sa.read_tile(1, 0, 0).unwrap();
    assert_eq!(t.shape(), (h as usize, w as usize));
    drop(sa);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn non_power_of_two_overviews_are_ignored() {
    let dir = temp_dir("ovr3");
    for (name, factors, levels) in [("a.tif", &[3][..], 1), ("b.tif", &[2, 8][..], 2)] {
        let f = make_fixture(&dir, name, 1000, 700, DataType::U16, 1, false, None, None);
        let mut ds = gdal::Dataset::open(&f.path).unwrap();
        ds.build_overviews("NEAREST", factors, &[]).unwrap();
        drop(ds);
        let s = GdalSource::open(&f.path, 1).unwrap();
        assert_eq!(s.levels(), levels, "{factors:?}");
        let t = s.read_tile(0, 1, 1).unwrap();
        assert_eq!(t.get_f64(0, 1), 513.0);
        if levels == 2 {
            assert_eq!(s.read_tile(1, 0, 0).unwrap().shape(), (350, 500));
        }
        assert!(s.read_tile(levels, 0, 0).is_err());
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn vector_file_is_not_a_product() {
    let dir = temp_dir("vec");
    let path = dir.join("pts.geojson");
    std::fs::write(
        &path,
        r#"{"type":"FeatureCollection","features":[{"type":"Feature","properties":{},"geometry":{"type":"Point","coordinates":[1,2]}}]}"#,
    )
    .unwrap();
    assert!(
        gdal::Dataset::open(&path).is_ok(),
        "GDAL opens it as vector"
    );
    let mut reg = snap_io::Registry::default();
    snap_io_gdal::register(&mut reg);
    match reg.open(&path) {
        Err(IoError::NoReader(_)) | Err(IoError::Open { .. }) => {}
        Err(e) => panic!("unexpected error {e}"),
        Ok(p) => panic!("vector opened as product with {} bands", p.bands.len()),
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn open_missing_file_is_ioerror_not_stderr() {
    match GdalSource::open(std::path::Path::new("nope.tif"), 1) {
        Err(IoError::Open { msg, .. }) => assert!(!msg.is_empty()),
        _ => panic!("expected IoError::Open"),
    }
}

fn make_paletted(path: &std::path::Path) {
    use gdal::raster::{
        Buffer, ColorEntry, ColorInterpretation, ColorTable, PaletteInterpretation,
    };
    let (w, h) = (1100usize, 1100usize);
    let drv = gdal::DriverManager::get_driver_by_name("GTiff").unwrap();
    let ds = drv.create_with_band_type::<u8, _>(path, w, h, 1).unwrap();
    // classes 1..=4 in 2x2 blocks: the mean of a block (2.5) is no class
    let data: Vec<u8> = (0..w * h)
        .map(|i| (1 + (i % w) % 2 + 2 * ((i / w) % 2)) as u8)
        .collect();
    let mut b = ds.rasterband(1).unwrap();
    b.write((0, 0), (w, h), &mut Buffer::new((w, h), data))
        .unwrap();
    b.set_no_data_value(Some(0.0)).unwrap();
    let mut ct = ColorTable::new(PaletteInterpretation::Rgba);
    for (i, c) in [
        [0, 0, 0, 0],
        [255, 255, 0, 255],
        [211, 211, 211, 255],
        [0, 100, 0, 255],
        [50, 205, 50, 255],
    ]
    .iter()
    .enumerate()
    {
        ct.set_color_entry(i as u16, &ColorEntry::rgba(c[0], c[1], c[2], c[3]));
    }
    b.set_color_table(&ct);
    b.set_color_interpretation(ColorInterpretation::PaletteIndex)
        .unwrap();
}

#[test]
fn paletted_band_gets_color_table_and_nearest_pyramid() {
    use snap_io::ProductReader;
    let dir = temp_dir("palette");
    let path = dir.join("classes.tif");
    make_paletted(&path);
    let p = snap_io_gdal::GeoTiffReader.read(&path).unwrap();
    let ct = p.bands[0].color_table.as_ref().expect("color table");
    // a GeoTIFF colormap always has 256 entries; unused ones come back opaque black
    assert_eq!(ct.entries.len(), 256);
    assert_eq!(ct.entries[5], [0, 0, 0, 255]);
    assert_eq!(ct.entries[0], [0, 0, 0, 0]);
    assert_eq!(ct.entries[1], [255, 255, 0, 255]);
    assert_eq!(ct.entries[4], [50, 205, 50, 255]);
    // the 1100 px image has no overviews, so level 1 is computed: top-left pixel, not the mean
    assert_eq!(
        p.bands[0].source.read_tile(1, 0, 0).unwrap().get_f64(0, 0),
        1.0
    );
    // a plain band has neither
    let g = make_fixture(
        &dir,
        "plain.tif",
        600,
        600,
        DataType::U8,
        1,
        false,
        None,
        None,
    );
    let q = snap_io_gdal::GeoTiffReader.read(&g.path).unwrap();
    assert!(q.bands[0].color_table.is_none());
    drop((p, q)); // Windows cannot delete files GDAL still holds open
    std::fs::remove_dir_all(dir).unwrap();
}
