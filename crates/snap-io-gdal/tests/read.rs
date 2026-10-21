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
fn open_missing_file_is_ioerror_not_stderr() {
    match GdalSource::open(std::path::Path::new("nope.tif"), 1) {
        Err(IoError::Open { msg, .. }) => assert!(!msg.is_empty()),
        _ => panic!("expected IoError::Open"),
    }
}
