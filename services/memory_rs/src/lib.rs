//! `memory_rs`: transport-neutral models, service, and store for the
//! `AgentSpace` text-first memory system, plus the `memory` CLI built on top.
//!
//! The crate exposes the same memory contract through local and HTTP clients.

pub mod cli;
pub mod client;
pub mod command_runner;
pub mod direct_client;
pub mod error;
pub mod frontmatter;
pub mod fs_store;
pub mod http_client;
pub mod links;
pub mod model;
pub mod path;
pub mod run_stream;
pub mod server;
pub mod service;
pub mod store;
pub mod wire;
