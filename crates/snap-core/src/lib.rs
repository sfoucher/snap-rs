mod cache;
mod color_table;
mod datatype;
mod error;
mod pyramid;
mod raster;
mod tile;

pub use cache::{TileCache, TileKey, TileState};
pub use color_table::ColorTable;
pub use datatype::{DataType, Scaling};
pub use error::{CoreError, GeoError};
pub use pyramid::{downsample2, downsample2_nodata, Pyramid};
pub use raster::{MemorySource, RasterSource};
pub use tile::{level_size, tile_count, tile_rect, Rect, Tile, TILE};
mod geocoding;
mod metadata;
mod product;
mod stats;

pub use geocoding::{Crs, CrsGeoCoding, GeoCoding, GeoPos, NoGeoCoding, PixelPos};
pub use metadata::{MetadataAttribute, MetadataElement, MetadataValue};
pub use product::{Band, Mask, Product, ProductId};
pub use stats::{compute_stats, compute_stats_coarsest, BandStats};
