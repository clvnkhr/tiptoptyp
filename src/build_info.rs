//! Compiled-in identity: no runtime Git commands, disk reads or timers.
#[cfg(feature = "production")]
pub(crate) const APP_NAME: &str = "tiptoptyp";

#[cfg(not(feature = "production"))]
pub(crate) const APP_NAME: &str = "tiptoptyp Dev";

/// Keep changing build labels out of the large UI codegen units. This returns
/// a compiled literal and performs no allocation, Git command, disk I/O or timer.
#[inline(never)]
pub(crate) fn version() -> &'static str {
    #[cfg(feature = "production")]
    {
        concat!(
            "tiptoptyp ",
            env!("CARGO_PKG_VERSION"),
            " (",
            env!("TIPTOPTYP_BUILD_ID"),
            ")"
        )
    }
    #[cfg(not(feature = "production"))]
    {
        concat!(
            "tiptoptyp Dev ",
            env!("CARGO_PKG_VERSION"),
            " (",
            env!("TIPTOPTYP_BUILD_ID"),
            ")"
        )
    }
}
