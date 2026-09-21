//! Compiled-in identity: no runtime Git commands, disk reads or timers.
#[cfg(feature = "production")]
pub(crate) const APP_NAME: &str = "tiptoptyp";

#[cfg(not(feature = "production"))]
pub(crate) const APP_NAME: &str = "tiptoptyp Dev";

#[cfg(feature = "production")]
pub(crate) const VERSION: &str = concat!(
    "tiptoptyp ",
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("TIPTOPTYP_BUILD_ID"),
    ")"
);

#[cfg(not(feature = "production"))]
pub(crate) const VERSION: &str = concat!(
    "tiptoptyp Dev ",
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("TIPTOPTYP_BUILD_ID"),
    ")"
);
