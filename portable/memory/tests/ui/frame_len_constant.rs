#![no_std]
use oer_memory::FrameLen;
#[cfg(not(ui_reject))]
pub const LENGTH: FrameLen<4> = FrameLen::constant::<4>();
#[cfg(ui_reject)]
pub const LENGTH: FrameLen<4> = FrameLen::constant::<5>();
// ui-error@outside: E0080: evaluation panicked: frame length exceeds capacity
