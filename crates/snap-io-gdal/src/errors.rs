use gdal::errors::CplErrType;
use std::cell::RefCell;
use std::sync::Once;

thread_local! {
    static LAST: RefCell<String> = const { RefCell::new(String::new()) };
}

/// Route GDAL's CPLError to `log` and remember the message; idempotent.
pub fn init_error_handler() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        gdal::config::set_error_handler(|ty, _num, msg| {
            match ty {
                CplErrType::Warning => log::warn!("GDAL: {msg}"),
                CplErrType::Failure | CplErrType::Fatal => log::error!("GDAL: {msg}"),
                _ => return,
            }
            LAST.with(|l| *l.borrow_mut() = msg.to_string());
        });
    });
}

/// Take (and clear) the last GDAL warning/error message on this thread; empty if none.
pub(crate) fn last_gdal_error() -> String {
    LAST.with(|l| std::mem::take(&mut *l.borrow_mut()))
}

/// Open read-only with VERBOSE_ERROR so failures raise a CPLError (the default flags stay silent).
pub(crate) fn open_dataset(path: &std::path::Path) -> gdal::errors::Result<gdal::Dataset> {
    use gdal::{DatasetOptions, GdalOpenFlags};
    clear_gdal_error();
    gdal::Dataset::open_ex(
        path,
        DatasetOptions {
            open_flags: GdalOpenFlags::GDAL_OF_READONLY | GdalOpenFlags::GDAL_OF_VERBOSE_ERROR,
            ..Default::default()
        },
    )
}

/// Call right before a GDAL call that can fail, so stale text is never reported.
pub(crate) fn clear_gdal_error() {
    LAST.with(|l| l.borrow_mut().clear());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_message_is_not_reused() {
        init_error_handler();
        let bad = std::env::temp_dir().join(format!("snap-rs-bad-{}.tif", std::process::id()));
        std::fs::write(&bad, b"not a raster").unwrap();
        let _ = open_dataset(&bad);
        // handler is process-global and tests run in parallel, but LAST is per-thread
        assert!(!last_gdal_error().is_empty());
        assert!(last_gdal_error().is_empty(), "take clears");
        let _ = open_dataset(&bad);
        clear_gdal_error();
        std::fs::remove_file(&bad).unwrap();
        assert!(last_gdal_error().is_empty());
    }
}
