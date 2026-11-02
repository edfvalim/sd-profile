//! Profiling of SD and microSD cards.
//!
//! The binary in `main.rs` is a thin shell around [`app::run`].

pub mod app;
pub mod cli;
mod disk;
pub mod error;
pub mod model;
mod platform;
mod render;

#[cfg(test)]
mod test_support;
