// Keep production unwraps checked without restricting integration-test helpers.
#![cfg_attr(not(test), warn(clippy::unwrap_used))]

pub mod attention;
pub mod cli;
pub mod config;
pub mod launch;
pub mod model;
pub mod notify;
pub mod pipeline;
pub mod project;
pub mod scan;
pub mod source;
pub mod state;
pub mod status;
mod storage;
pub mod tmux;
