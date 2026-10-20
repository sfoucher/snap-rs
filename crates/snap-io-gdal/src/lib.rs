mod bundle;
mod errors;
mod generic_reader;
mod geotiff_reader;
mod geotiff_writer;
mod metadata;
mod source;

pub use bundle::use_bundled_data;
pub use errors::init_error_handler;
pub use generic_reader::GdalGenericReader;
pub use geotiff_reader::GeoTiffReader;
pub use geotiff_writer::GeoTiffWriter;
pub use source::GdalSource;

pub fn register(reg: &mut snap_io::Registry) {
    reg.add_reader(Box::new(GeoTiffReader));
    reg.add_reader(Box::new(GdalGenericReader));
    reg.add_writer(Box::new(GeoTiffWriter));
}
