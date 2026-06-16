//! Metamorphosis MCP Server — Model Context Protocol integration.
//!
//! Exposes metamorphosis SQL rewriting, suggestion, verification,
//! and schema extraction as MCP tools over stdio transport.

pub mod server;
pub mod tools;
pub mod types;

pub use server::run_stdio;

#[cfg(test)]
mod tests {
    use super::server::MetamorphosisServer;

    #[test]
    fn tool_router_registers_six_tools() {
        let router = MetamorphosisServer::tool_router();
        let tools = router.list_all();
        let names: Vec<_> = tools.iter().map(|t| t.name.clone()).collect();
        assert_eq!(tools.len(), 6, "expected 6 tools, got: {names:?}");
    }
}
