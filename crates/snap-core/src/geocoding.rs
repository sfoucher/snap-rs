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
