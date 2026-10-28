#![no_std]
#![forbid(unsafe_code)]

//! W3 image's bounded observations and console report.

pub mod beacon;
pub mod bridge;
// The station service's observations, counted only in the trace class.
#[cfg(oer_trace)]
pub mod collector;
pub mod echo;
pub mod icmp;
pub mod observation;
pub mod policy;
pub mod probe;
pub mod report;

pub mod loss;
