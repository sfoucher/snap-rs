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

/// Last GDAL warning/error message on this thread; empty if none.
pub(crate) fn last_gdal_error() -> String {
    LAST.with(|l| l.borrow().clone())
}
