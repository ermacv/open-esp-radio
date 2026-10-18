//! i validation register and hardware operations.

#[cfg(any(test, feature = "validation-probes"))]
#[cfg(feature = "validation-probes")]
pub(crate) mod ed_event;

#[cfg(any(test, feature = "validation-probes"))]
pub(crate) mod event_status;

#[cfg(any(test, feature = "validation-probes"))]
pub mod route_retrigger;

#[cfg(feature = "validation-probes")]
pub mod coex_trace;
