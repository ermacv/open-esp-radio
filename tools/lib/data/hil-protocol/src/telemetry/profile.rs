//! A statistical program-counter profile of the image's harts.
//!
//! The host arms the profile before a workload; the workload opens and
//! closes its measured window, and the image samples the interrupted program
//! counter and return address of each selected hart at a fixed period only
//! while it is armed and the window is open. After the window the host pages
//! out the raw samples and symbolizes them against the image's ELF.

use heapless::Vec;
use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

/// Samples one [`ProfileSamplesPage`] carries.
pub const PROFILE_SAMPLE_PAGE: usize = 48;

/// Which harts a profile samples.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum ProfileHarts {
    Both,
    Core0,
    Core1,
}

impl ProfileHarts {
    /// Whether `hart` is sampled.
    pub const fn includes(self, hart: usize) -> bool {
        match self {
            Self::Both => hart < 2,
            Self::Core0 => hart == 0,
            Self::Core1 => hart == 1,
        }
    }
}

/// What the host asks of the profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum ProfileControl {
    /// Discard the previous profile and sample the next workload window.
    Arm { harts: ProfileHarts, period_us: u32 },
    /// Stop sampling.
    Disarm,
    /// Report the profile's state.
    Status,
}

/// The profile's state.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ProfileStatus {
    pub armed: bool,
    /// The workload's window is open: its samples are not complete yet.
    pub open: bool,
    pub harts: ProfileHarts,
    pub period_us: u32,
    /// The closed window's length.
    pub window_us: u32,
    /// Samples each hart can retain.
    pub capacity: u32,
    /// Samples each hart retained.
    pub samples: [u32; 2],
    /// Samples each hart dropped once full.
    pub overflow: [u32; 2],
}

/// Retained samples of one hart from `first`, at most one page.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ProfileSamplesPage {
    pub hart: u8,
    pub first: u32,
    /// The hart's retained samples in all.
    pub total: u32,
    /// `(pc, ra)`: the interrupted instruction and return address.
    pub samples: Vec<(u32, u32), PROFILE_SAMPLE_PAGE>,
}
