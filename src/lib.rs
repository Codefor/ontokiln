//! OntoKiln: one immutable in-memory Pack v4 compiler and reasoner.
//! Applications own the persistence, deployment and lifetime of the engine.
pub mod engine;
pub mod identity;
pub mod query;
pub mod rdf_graph;
pub mod shacl;
