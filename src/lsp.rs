//! Shared wire contracts, independent of any server lifecycle.
pub(crate) mod protocol;
pub(crate) mod transport;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Generation(pub u64);
