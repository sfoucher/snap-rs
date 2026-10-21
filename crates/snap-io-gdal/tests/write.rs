mod fixtures;
use fixtures::*;
use gdal::Dataset;
use snap_core::{
    Band, CoreError, DataType, MemorySource, Product, RasterSource, Rect, Scaling, Tile, TileCache,
    TileKey,
};
use snap_io::{Compression, ProductWriter, Registry, SubsetRegion, WriteOpts};
use std::path::Path;
use std::sync::Arc;

fn px(cache: &TileCache, p: &Product, band: usize, x: u32, y: u32) -> f64 {
    let b = &p.bands[band];
    let key = TileKey {
        product: p.id,
        band: b.name.clone(),
        level: 0,
        tx: x / 512,
        ty: y / 512,
    };
    let t = cache.get_blocking(&key, &b.source).unwrap();
    t.get_f64((y % 512) as usize, (x % 512) as usize)
}

fn gt_of(path: &Path) -> [f64; 6] {
    Dataset::open(path).unwrap().geo_transform().unwrap()
}

fn setup(tag: &str) -> (std::path::PathBuf, Fixture, Registry, Arc<TileCache>) {
    let dir = temp_dir(tag);
    let f = make_fixture(
        &dir,
        "in.tif",
        1000,
        700,
        DataType::I16,
        3,
        false,
        Some(-9999.0),
        Some(32618),
    );
    let mut reg = Registry::default();
    snap_io_gdal::register(&mut reg);
    (dir, f, reg, TileCache::new(256 << 20).0)
}

fn export(reg: &Registry, cache: &TileCache, src: &Product, out: &Path, opts: &WriteOpts) {
    reg.writer("GeoTIFF")
        .unwrap()
        .write(src, out, opts, cache)
        .unwrap();
}

#[test]
fn write_read_roundtrip() {
    let (dir, f, reg, cache) = setup("wr");
    let src = reg.open(&f.path).unwrap();
    let out = dir.join("out.tif");
    export(&reg, &cache, &src, &out, &WriteOpts::default());
    let back = reg.open(&out).unwrap();
    assert_eq!((back.width, back.height), (1000, 700));
    assert_eq!(back.bands.len(), 3);
    assert_eq!(back.bands[0].no_data, Some(-9999.0));
    assert_eq!(back.bands[1].data_type, DataType::I16);
    assert_eq!(px(&cache, &back, 1, 10, 10), px(&cache, &src, 1, 10, 10));
    assert_eq!(
        px(&cache, &back, 1, 10, 10),
        (10 * 1000 + 10 + 100) as f64 % 1000.0
    );
    assert_eq!(
        px(&cache, &back, 2, 999, 699),
        px(&cache, &src, 2, 999, 699)
    );
    assert_eq!(gt_of(&out), f.gt);
    let ds = Dataset::open(&out).unwrap();
    assert_eq!(ds.rasterband(1).unwrap().overview_count().unwrap(), 1);
    drop(ds);
    drop(back);
    drop(src);
    drop(cache);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn subset_shifts_geotransform() {
    let (dir, f, reg, cache) = setup("ws");
    let src = reg.open(&f.path).unwrap();
    let out = dir.join("sub.tif");
    let opts = WriteOpts {
        subset: Some(SubsetRegion {
            rect: Rect {
                x: 100,
                y: 50,
                w: 600,
                h: 600,
            },
            bands: vec![],
        }),
        ..Default::default()
    };
    export(&reg, &cache, &src, &out, &opts);
    let back = reg.open(&out).unwrap();
    assert_eq!((back.width, back.height), (600, 600));
    let gt = gt_of(&out);
    assert_eq!(gt[0], f.gt[0] + 100.0 * f.gt[1]);
    assert_eq!(gt[3], f.gt[3] + 50.0 * f.gt[5]);
    assert_eq!((gt[1], gt[5]), (f.gt[1], f.gt[5]));
    assert_eq!(px(&cache, &back, 0, 0, 0), px(&cache, &src, 0, 100, 50));
    assert_eq!(
        px(&cache, &back, 0, 599, 599),
        px(&cache, &src, 0, 699, 649)
    );
    drop(back);
    drop(src);
    drop(cache);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn subset_band_selection_and_order() {
    let (dir, f, reg, cache) = setup("wb");
    let src = reg.open(&f.path).unwrap();
    let out = dir.join("bands.tif");
    let opts = WriteOpts {
        subset: Some(SubsetRegion {
            rect: Rect {
                x: 0,
                y: 0,
                w: 1000,
                h: 700,
            },
            bands: vec!["band_3".into(), "band_1".into()],
        }),
        ..Default::default()
    };
    export(&reg, &cache, &src, &out, &opts);
    let back = reg.open(&out).unwrap();
    assert_eq!(back.bands.len(), 2);
    assert_eq!(back.bands[0].name, "band_3");
    let (a, b3, b1) = (
        px(&cache, &back, 0, 5, 5),
        px(&cache, &src, 2, 5, 5),
        px(&cache, &src, 0, 5, 5),
    );
    assert_eq!(a, b3);
    assert_ne!(a, b1);
    drop(back);
    drop(src);
    drop(cache);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn invalid_subset_rejected() {
    let (dir, f, reg, cache) = setup("wi");
    let src = reg.open(&f.path).unwrap();
    let w = reg.writer("GeoTIFF").unwrap();
    let out = dir.join("x.tif");
    for (rect, bands) in [
        (
            Rect {
                x: 900,
                y: 0,
                w: 200,
                h: 10,
            },
            vec![],
        ),
        (
            Rect {
                x: 0,
                y: 0,
                w: 0,
                h: 10,
            },
            vec![],
        ),
        (
            Rect {
                x: 0,
                y: 0,
                w: 10,
                h: 10,
            },
            vec!["nope".to_string()],
        ),
    ] {
        let opts = WriteOpts {
            subset: Some(SubsetRegion { rect, bands }),
            ..Default::default()
        };
        assert!(matches!(
            w.write(&src, &out, &opts, &cache),
            Err(snap_io::IoError::Write(_))
        ));
    }
    drop(src);
    drop(cache);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn cog_and_compression_reopen() {
    let (dir, f, reg, cache) = setup("wc");
    let src = reg.open(&f.path).unwrap();
    let out = dir.join("cog.tif");
    let opts = WriteOpts {
        cog: true,
        compression: Compression::Deflate,
        ..Default::default()
    };
    export(&reg, &cache, &src, &out, &opts);
    assert!(!dir.join("cog.tif.tmp.tif").exists());
    let back = reg.open(&out).unwrap();
    assert_eq!((back.width, back.height, back.bands.len()), (1000, 700, 3));
    assert_eq!(
        px(&cache, &back, 2, 700, 600),
        px(&cache, &src, 2, 700, 600)
    );
    let ds = Dataset::open(&out).unwrap();
    let b = ds.rasterband(3).unwrap();
    assert!(b.overview_count().unwrap() >= 1);
    // AVERAGE, not the COG default CUBIC: overview (400,2) straddles the value wrap and is the 2x2 mean (within 1)
    let ov: Vec<i16> = b
        .overview(0)
        .unwrap()
        .read_as((400, 2), (1, 1), (1, 1), None)
        .unwrap()
        .into_shape_and_vec()
        .1;
    let mean = (px(&cache, &src, 2, 800, 4)
        + px(&cache, &src, 2, 801, 4)
        + px(&cache, &src, 2, 800, 5)
        + px(&cache, &src, 2, 801, 5))
        / 4.0;
    assert!(
        (f64::from(ov[0]) - mean).abs() <= 1.0,
        "{} vs {mean}",
        ov[0]
    );
    drop(ds);
    drop(back);
    drop(src);
    drop(cache);
    std::fs::remove_dir_all(dir).unwrap();
}

fn mem_band(name: &str, no_data: Option<f64>, source: Arc<dyn RasterSource>) -> Band {
    let (width, height) = source.size();
    Band {
        name: name.into(),
        data_type: source.data_type(),
        width,
        height,
        no_data,
        scaling: Scaling {
            factor: 1.0,
            offset: 0.0,
            log10: false,
        },
        unit: None,
        valid_pixel_expr: None,
        description: None,
        source,
        stats: Default::default(),
        color_table: None,
    }
}

struct Broken;

impl RasterSource for Broken {
    fn data_type(&self) -> DataType {
        DataType::F32
    }
    fn size(&self) -> (u32, u32) {
        (600, 600)
    }
    fn levels(&self) -> u8 {
        1
    }
    fn read_tile(&self, _: u8, tx: u32, _: u32) -> Result<Tile, CoreError> {
        if tx == 0 {
            Ok(Tile::F32(ndarray::Array2::zeros((512, 512))))
        } else {
            Err(CoreError::Other("disk on fire".into()))
        }
    }
}

#[test]
fn failed_export_keeps_existing_target() {
    let dir = temp_dir("wfail");
    let out = dir.join("keep.tif");
    std::fs::write(&out, b"precious").unwrap();
    let mut p = Product::new("p", "t", 600, 600);
    p.add_band(mem_band("b", None, Arc::new(Broken))).unwrap();
    let cache = TileCache::new(64 << 20).0;
    for cog in [false, true] {
        let opts = WriteOpts {
            cog,
            ..Default::default()
        };
        assert!(snap_io_gdal::GeoTiffWriter
            .write(&p, &out, &opts, &cache)
            .is_err());
        assert_eq!(std::fs::read(&out).unwrap(), b"precious");
    }
    let left: Vec<_> = std::fs::read_dir(&dir).unwrap().collect();
    assert_eq!(left.len(), 1, "temp files left behind");
    drop(cache);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn mixed_nodata_bands_rejected() {
    let dir = temp_dir("wnd");
    let src = || -> Arc<dyn RasterSource> {
        Arc::new(MemorySource::new(ndarray::Array2::<f32>::zeros((4, 4))))
    };
    let cache = TileCache::new(64 << 20).0;
    for (a, b) in [(Some(0.0), Some(1.0)), (Some(0.0), None)] {
        let mut p = Product::new("p", "t", 4, 4);
        p.add_band(mem_band("a", a, src())).unwrap();
        p.add_band(mem_band("b", b, src())).unwrap();
        match snap_io_gdal::GeoTiffWriter.write(
            &p,
            &dir.join("x.tif"),
            &WriteOpts::default(),
            &cache,
        ) {
            Err(snap_io::IoError::Write(m)) => assert!(m.contains("different no-data"), "{m}"),
            _ => panic!("expected a no-data mismatch error"),
        }
    }
    assert!(!dir.join("x.tif").exists());
    std::fs::remove_dir_all(dir).unwrap();
}
