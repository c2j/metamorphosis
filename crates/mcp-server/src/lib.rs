//! Metamorphosis MCP Server — Model Context Protocol integration.
//!
//! Exposes metamorphosis SQL rewriting, suggestion, verification,
//! and schema extraction as MCP tools over stdio transport.

pub mod server;
pub mod tools;
pub mod types;

pub use server::run_stdio;
