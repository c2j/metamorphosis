//! MCP server handler and stdio transport.

use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::{ServerHandler, ServiceExt};

use crate::tools;
use crate::types::*;

/// Stateless MCP server — all engine operations are performed per-request.
pub struct MetamorphosisServer {
    /// Tool routing table populated by `#[rmcp::tool_router]`.
    pub(crate) tool_router: ToolRouter<Self>,
}

impl Default for MetamorphosisServer {
    fn default() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }
}

#[rmcp::tool_router(vis = "pub(crate)")]
impl MetamorphosisServer {
    #[rmcp::tool(
        name = "rewrite_sql",
        description = "Rewrite SQL using Safe and Conditional semantic rules. \
            Returns rewritten SQL statements with match diagnostics."
    )]
    async fn rewrite_sql(
        &self,
        Parameters(params): Parameters<SqlParams>,
    ) -> String {
        match tools::rewrite_sql(params) {
            Ok(result) => serde_json::to_string_pretty(&result)
                .unwrap_or_else(|e| format!("{{\"error\": \"serialization failed: {e}\"}}")),
            Err(e) => serde_json::to_string_pretty(&ErrorResponse { error: e })
                .unwrap_or_else(|_| r#"{"error": "unknown error"}"#.to_string()),
        }
    }

    #[rmcp::tool(
        name = "suggest_probes",
        description = "Generate data quality probe SQL suggestions using Manual-level rules. \
            Returns probe SQL statements with confidence levels and match diagnostics."
    )]
    async fn suggest_probes(
        &self,
        Parameters(params): Parameters<SqlParams>,
    ) -> String {
        match tools::suggest_probes(params) {
            Ok(result) => serde_json::to_string_pretty(&result)
                .unwrap_or_else(|e| format!("{{\"error\": \"serialization failed: {e}\"}}")),
            Err(e) => serde_json::to_string_pretty(&ErrorResponse { error: e })
                .unwrap_or_else(|_| r#"{"error": "unknown error"}"#.to_string()),
        }
    }

    #[rmcp::tool(
        name = "list_rules",
        description = "List all available rewrite rules with their metadata: \
            id, description, category, and safety level."
    )]
    async fn list_rules(&self) -> String {
        let result = tools::list_rules();
        serde_json::to_string_pretty(&result)
            .unwrap_or_else(|e| format!("{{\"error\": \"serialization failed: {e}\"}}"))
    }

    #[rmcp::tool(
        name = "verify_equivalence",
        description = "Verify semantic equivalence of two SQL queries using Z3 SMT solver. \
            Supports two engines: 'qed' (default, rich schema constraints) and 'verieql' (bounded verification). \
            Schema is required."
    )]
    async fn verify_equivalence(
        &self,
        Parameters(params): Parameters<VerifyParams>,
    ) -> String {
        match tools::verify_equivalence(params) {
            Ok(result) => serde_json::to_string_pretty(&result)
                .unwrap_or_else(|e| format!("{{\"error\": \"serialization failed: {e}\"}}")),
            Err(e) => serde_json::to_string_pretty(&ErrorResponse { error: e })
                .unwrap_or_else(|_| r#"{"error": "unknown error"}"#.to_string()),
        }
    }

    #[rmcp::tool(
        name = "extract_schema",
        description = "Extract table schema (column names and types) from a directory \
            of DDL SQL files. Returns a JSON schema map suitable for use with \
            rewrite_sql and suggest_probes."
    )]
    async fn extract_schema(
        &self,
        Parameters(params): Parameters<ExtractSchemaParams>,
    ) -> String {
        match tools::extract_schema(params) {
            Ok(result) => serde_json::to_string_pretty(&result)
                .unwrap_or_else(|e| format!("{{\"error\": \"serialization failed: {e}\"}}")),
            Err(e) => serde_json::to_string_pretty(&ErrorResponse { error: e })
                .unwrap_or_else(|_| r#"{"error": "unknown error"}"#.to_string()),
        }
    }
}

#[rmcp::tool_handler]
impl ServerHandler for MetamorphosisServer {}

/// Blocks until the transport is closed (e.g., stdin EOF).
pub async fn run_stdio() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let server = MetamorphosisServer::default();
    let transport = rmcp::transport::io::stdio();
    let service = server.serve(transport).await?;
    service.waiting().await?;
    Ok(())
}
