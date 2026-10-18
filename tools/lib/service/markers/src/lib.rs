//! The one recogniser of the vendor `SOURCE` citation blocks the
//! repository's tools read ([`source`]): their chips and the place a
//! citation is attributed to (vendor provenance registers them).
//!
//! It depends on nothing, so vendor provenance and other readers share it
//! without linking each other.
#![forbid(unsafe_code)]

pub mod source;
