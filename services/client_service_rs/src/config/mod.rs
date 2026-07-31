//! Declarative configuration for the `ConfigDocument` desired state.
//!
//! This module owns the single strict `ConfigDocument` schema, its canonical
//! serialization, an opaque snapshot store, a write-only secret store, graph
//! validation, planning, and lazy secret resolution.
//!
//! The `ConfigDocument` is the one authoritative record for configuration: it
//! is the YAML schema, the active in-memory config, the validation input, the
//! UI mutation target, and the canonical serialization model. There are no
//! parallel persistence models for configuration fields.

pub mod adapter;
pub mod bundle;
pub mod canonical;
pub mod document;
pub mod error;
pub mod loader;
pub mod plan;
pub mod resolver;
pub mod secrets;
pub mod skill_validation;
pub mod snapshot;
pub mod state;
pub mod validate;
pub mod value;

pub use document::{
    Agent, AggregateManifest, ConfigDocument, ConfigSpec, Connection, Gateway, KernelConfig,
    SecretDeclaration, Skill,
};
pub use error::{ConfigError, ValidationIssue};
pub use value::{ConfigValue, SecretName};
