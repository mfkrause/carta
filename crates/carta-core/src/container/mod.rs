//! Deterministic ZIP packaging and XML emission for container formats.

pub mod xml;
#[cfg(feature = "container")]
#[cfg_attr(docsrs, doc(cfg(feature = "container")))]
pub mod zip;
