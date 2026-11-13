//! Generic RDF execution shared by the CLI and application adapters.
//!
//! Callers own vocabulary, rule selection and application protocols. Full
//! requests must run inside a supervised worker when inputs are untrusted.
pub mod api;
pub mod disk;
pub mod engine;
pub mod full;
pub mod full_artifact;
pub mod full_execution;
mod full_rules;
pub mod full_snapshot;
pub mod identity;
pub mod query;
pub mod rdf_graph;
pub mod session;
pub mod shacl;
mod shacl_sqlite;
