//! The commands behind `cargo fw`: build, flash and run the staged images
//! of `firmware/apps` on the stand's boards (`FW.md`).

mod command;
mod flash;
mod image;
mod port;
mod power;
pub mod probe;
mod w1;

pub use command::{build, flash_image, list_boards, monitor, power_cycle, root, router_ready, run};
