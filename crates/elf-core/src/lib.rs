//! `elf-core`: business logic of the Elf desktop assistant.
//!
//! This crate has no Tauri or Windows dependency. External capabilities are
//! injected through the traits in [`ports`]; all time-dependent logic goes
//! through [`ports::Clock`].

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![deny(clippy::unwrap_used, clippy::expect_used)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod model;
pub mod policy;
pub mod ports;
pub mod schema;
pub mod storage;
pub mod task;
