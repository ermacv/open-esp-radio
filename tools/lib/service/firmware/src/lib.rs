//! The service behind `cargo fw`: build, flash and run the staged images
//! of `firmware/apps` on the stand's boards (README.md).

mod command;
mod encode;
mod flash;
mod gates;
mod image;
mod lock;
mod pack;
mod placement;
mod port;
mod power;
mod w1;

pub use command::{build, flash_image, list_boards, monitor, power_cycle, root, router_ready, run};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
