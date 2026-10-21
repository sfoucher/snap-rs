#[derive(Copy, Clone, Debug, PartialEq)]
pub struct GeoPos {
    pub lon: f64,
    pub lat: f64,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct PixelPos {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Crs {
    pub wkt: String,
    pub epsg: Option<u32>,
    pub name: String,
}

pub trait GeoCoding: Send + Sync {
    fn pixel_to_geo(&self, x: f64, y: f64) -> Option<GeoPos>;
    fn geo_to_pixel(&self, pos: GeoPos) -> Option<PixelPos>;
    fn crs(&self) -> Option<&Crs>;
    fn pixel_to_crs(&self, x: f64, y: f64) -> Option<(f64, f64)>;
}

pub struct NoGeoCoding;

impl GeoCoding for NoGeoCoding {
    fn pixel_to_geo(&self, _: f64, _: f64) -> Option<GeoPos> {
        None
    }
    fn geo_to_pixel(&self, _: GeoPos) -> Option<PixelPos> {
        None
    }
    fn crs(&self) -> Option<&Crs> {
        None
    }
    fn pixel_to_crs(&self, _: f64, _: f64) -> Option<(f64, f64)> {
        None
    }
}

use crate::GeoError;
use proj::Proj;
use std::sync::Mutex;

// SAFETY: each Proj owns its own PROJ context and is only reached through the Mutex,
// so it is never used from two threads at once; moving it between threads is sound.
// Caveat: Proj's Drop also calls the global proj_cleanup(), which may run while another
// thread uses its own context; this relies on PROJ >= 9 guarding that shared state internally.
struct SendProj(Proj);
unsafe impl Send for SendProj {}

pub struct CrsGeoCoding {
    gt: [f64; 6],
    inv: [f64; 6],
    crs: Crs,
    to_geo: Mutex<SendProj>,
    from_geo: Mutex<SendProj>,
}

impl CrsGeoCoding {
    pub fn new(geotransform: [f64; 6], crs: Crs) -> Result<Self, GeoError> {
        let [a, b, c, d, e, f] = geotransform;
        let det = b * f - c * e;
        if det == 0.0 || !det.is_finite() {
            return Err(GeoError::SingularTransform);
        }
        let inv = [
            (c * d - a * f) / det,
            f / det,
            -c / det,
            (a * e - b * d) / det,
            -e / det,
            b / det,
        ];
        let mk = |from: &str, to: &str| {
            // proj 0.27 panics ("PROJ did not supply an error") for some CRSs, e.g. LOCAL_CS.
            std::panic::catch_unwind(|| Proj::new_known_crs(from, to, None))
                .map_err(|_| GeoError::Proj(format!("PROJ cannot transform {from} to {to}")))?
                .map(|p| Mutex::new(SendProj(p)))
                .map_err(|e| GeoError::Proj(e.to_string()))
        };
        Ok(Self {
            gt: geotransform,
            inv,
            to_geo: mk(&crs.wkt, "EPSG:4326")?,
            from_geo: mk("EPSG:4326", &crs.wkt)?,
            crs,
        })
    }
}

impl GeoCoding for CrsGeoCoding {
    fn pixel_to_geo(&self, x: f64, y: f64) -> Option<GeoPos> {
        let p = self.pixel_to_crs(x, y)?;
        let (lon, lat) = self.to_geo.lock().ok()?.0.convert(p).ok()?;
        (lon.is_finite() && lat.is_finite()).then_some(GeoPos { lon, lat })
    }
    fn geo_to_pixel(&self, pos: GeoPos) -> Option<PixelPos> {
        let (e, n) = self
            .from_geo
            .lock()
            .ok()?
            .0
            .convert((pos.lon, pos.lat))
            .ok()?;
        let [a, b, c, d, f, g] = self.inv;
        let (x, y) = (a + b * e + c * n, d + f * e + g * n);
        (x.is_finite() && y.is_finite()).then_some(PixelPos { x, y })
    }
    fn crs(&self) -> Option<&Crs> {
        Some(&self.crs)
    }
    fn pixel_to_crs(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        let g = &self.gt;
        Some((g[0] + g[1] * x + g[2] * y, g[3] + g[4] * x + g[5] * y))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GeoError;

    // `gdalsrsinfo -o wkt1 EPSG:32618`
    fn epsg_wkt(_epsg: u32) -> String {
        r#"PROJCS["WGS 84 / UTM zone 18N",GEOGCS["WGS 84",DATUM["WGS_1984",SPHEROID["WGS 84",6378137,298.257223563,AUTHORITY["EPSG","7030"]],AUTHORITY["EPSG","6326"]],PRIMEM["Greenwich",0,AUTHORITY["EPSG","8901"]],UNIT["degree",0.0174532925199433,AUTHORITY["EPSG","9122"]],AUTHORITY["EPSG","4326"]],PROJECTION["Transverse_Mercator"],PARAMETER["latitude_of_origin",0],PARAMETER["central_meridian",-75],PARAMETER["scale_factor",0.9996],PARAMETER["false_easting",500000],PARAMETER["false_northing",0],UNIT["metre",1,AUTHORITY["EPSG","9001"]],AXIS["Easting",EAST],AXIS["Northing",NORTH],AUTHORITY["EPSG","32618"]]"#.into()
    }

    fn utm18n(gt: [f64; 6]) -> Result<CrsGeoCoding, GeoError> {
        CrsGeoCoding::new(
            gt,
            Crs {
                wkt: epsg_wkt(32618),
                epsg: Some(32618),
                name: "WGS 84 / UTM zone 18N".into(),
            },
        )
    }

    #[test]
    fn untransformable_crs_is_error_not_panic() {
        let crs = Crs {
            wkt: r#"LOCAL_CS["arbitrary",UNIT["metre",1],AXIS["X",EAST],AXIS["Y",NORTH]]"#.into(),
            epsg: None,
            name: "arbitrary".into(),
        };
        let r = CrsGeoCoding::new([0.0, 1.0, 0.0, 0.0, 0.0, -1.0], crs);
        assert!(matches!(r, Err(GeoError::Proj(_))));
    }

    #[test]
    fn affine_roundtrip_utm18n() {
        let gc = utm18n([700_000.0, 10.0, 0.0, 5_030_000.0, 0.0, -10.0]).unwrap();
        assert_eq!(gc.pixel_to_crs(100.0, 50.0), Some((701_000.0, 5_029_500.0)));
        let g = gc.pixel_to_geo(100.0, 50.0).unwrap();
        // gdaltransform -s_srs EPSG:32618 -t_srs EPSG:4326 <<< "701000 5029500"
        assert!((g.lon - (-72.4322371741319)).abs() < 1e-6);
        assert!((g.lat - 45.3901490956265).abs() < 1e-6);
        let p = gc.geo_to_pixel(g).unwrap();
        assert!((p.x - 100.0).abs() < 1e-6 && (p.y - 50.0).abs() < 1e-6);
    }

    #[test]
    fn singular_transform_rejected() {
        let r = utm18n([700_000.0, 0.0, 0.0, 5_030_000.0, 0.0, -10.0]);
        assert!(matches!(r, Err(GeoError::SingularTransform)));
    }

    #[test]
    fn nogeocoding_is_none() {
        assert!(NoGeoCoding.pixel_to_geo(0.0, 0.0).is_none());
    }
}
