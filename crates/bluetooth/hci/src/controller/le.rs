//! LE-specific HCI codecs and reset-scoped advertising, scanning and test policy.
//!
//! These modules decode Host commands and retain portable policy. Radio execution
//! remains with the caller; shared command/response authority stays in `order`.

pub(crate) mod accept_list;
pub(crate) mod acl;
pub(crate) mod advertising;
pub(crate) mod data_length;
pub(crate) mod dtm;
pub(crate) mod peripheral;
pub(crate) mod scanning;
