//! Native in-memory RDF compilation and positive-rule materialization.
pub mod v4;
pub mod v4_materialize;
pub use v4::{Source, compile_sources};
pub use v4_materialize::{Rule, Materialization, materialize};
