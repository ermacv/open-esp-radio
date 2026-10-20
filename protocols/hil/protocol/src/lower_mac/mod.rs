//! Lower-MAC conformance on a dedicated diagnostic image.
#[cfg(feature = "lower-mac")]
mod payload;
#[cfg(feature = "lower-mac")]
pub use payload::*;
#[cfg(feature = "lower-mac")]
crate::messages! {
    endpoint Prepare = "lower-mac/prepare" => Prepared;
    topic Prepared = "lower-mac/prepared";
    endpoint RunCase = "lower-mac/case/run" => CaseTrace;
    topic CaseTrace = "lower-mac/case/trace";
}
#[cfg(all(feature = "registry", not(feature = "lower-mac")))]
pub const MESSAGES: &[crate::MessageInfo] = &[];
