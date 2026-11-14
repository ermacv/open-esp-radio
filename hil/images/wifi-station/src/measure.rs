//! What the image reads of its own work in each of its two builds: each
//! hart's counters (cycles, retired instructions, the boot hart's cache)
//! and idle account in both, and the library's observers beside them in
//! the trace build (`trace`). The measuring build reads its counters at a
//! window's two ends and nothing inside it.

#[cfg(not(feature = "trace"))]
use core::{future::Future, pin::Pin};

/// The boot hart's readers.
pub(crate) struct Hart0 {
    pub(crate) cpu: oer::cpu::Cpu0,
    /// The boot hart's sampler.
    #[cfg(feature = "trace")]
    pub(crate) observe: oer::observe::Hart0,
}

/// Hart 1's readers.
pub(crate) struct Hart1 {
    pub(crate) cpu: oer::cpu::Cpu1,
    /// Hart 1's sampler.
    #[cfg(feature = "trace")]
    pub(crate) observe: oer::observe::Hart1,
}

/// A named part of the image whose polls the trace counts.
#[cfg(feature = "trace")]
pub(crate) use oer::observe::Part;

/// A named part of the image: the measuring build counts no polls.
#[cfg(not(feature = "trace"))]
pub(crate) struct Part;

#[cfg(not(feature = "trace"))]
impl Part {
    /// The part named `name`; the measuring build keeps no name.
    pub(crate) fn claim(_name: &'static str) -> Result<Self, oer::Error> {
        Ok(Self)
    }

    /// `future`, polled as it is.
    pub(crate) fn polled<F: Future>(self, future: Pin<&mut F>) -> Pin<&mut F> {
        future
    }
}
