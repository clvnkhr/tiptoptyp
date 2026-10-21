//! Explicit DOI/arXiv lookup and reviewed BibTeX imports. All network/filesystem
//! entry points belong on workers; parsing/rekeying is bounded and deterministic.
mod bibtex;
mod identifier;
mod network;
mod storage;

#[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
pub(crate) use bibtex::parse_entry;
pub(crate) use bibtex::{rekey, validate_key};
pub(crate) use identifier::Identifier;
pub(crate) use network::lookup;
pub(crate) use storage::BibliographySnapshot;

use std::sync::atomic::{AtomicBool, Ordering};
const MAX_ENTRY_BYTES: usize = 256 * 1024;
const MAX_BIB_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Citation {
    pub(crate) identifier: Identifier,
    pub(crate) key: String,
    pub(crate) bibtex: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ImportResult {
    pub(crate) key: String,
    pub(crate) bibtex: String,
    pub(crate) added: bool,
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Acquire) {
        Err("Citation import cancelled.".into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
