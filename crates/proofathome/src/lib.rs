//! The Proof@home CLI and MCP server.
//!
//! This slice runs in local mode: problems come from a catalog directory and
//! claims, attempts and results stay in the home directory. Everything goes
//! through the [`backend::Backend`] trait, so a coordinator service can take
//! the place of [`local::LocalBackend`] later. The binary is a thin layer over
//! [`app::App`], which the MCP server and the `start` runner share.

pub mod agent;
pub mod app;
pub mod backend;
pub mod catalog;
pub mod check;
pub mod error;
pub mod fsutil;
pub mod git;
pub mod local;
pub mod mcp;
pub mod patch;
pub mod paths;
pub mod report;
pub mod runner;
pub mod setup;
pub mod time;
pub mod workspace;

pub use proofathome_core;
