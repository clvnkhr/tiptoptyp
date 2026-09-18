//! Compiled-in identity: no runtime Git commands, disk reads or timers.
pub(crate) const VERSION: &str = concat!(
    "tiptoptyp ",
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("TIPTOPTYP_BUILD_ID"),
    ")"
);
