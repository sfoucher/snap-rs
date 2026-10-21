/// Class colours (RGBA) of a paletted raster, indexed by pixel value.
#[derive(Clone, Debug, PartialEq)]
pub struct ColorTable {
    pub entries: Vec<[u8; 4]>,
}
