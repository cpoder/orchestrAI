//! Transport glue: wraps [`OrchestrAiMcp`] in either a streamable-HTTP
//! service (mounted on the axum router) or a stdio session (read
//! line-delimited JSON-RPC from stdin, write to stdout).

use std::sync::Arc;

use rmcp::{
    ServiceExt,
    transport::{
        io::stdio,
        streamable_http_server::{
            StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
        },
    },
};

use super::OrchestrAiMcp;

pub type McpService = StreamableHttpService<OrchestrAiMcp, LocalSessionManager>;

pub fn build_http_service() -> McpService {
    StreamableHttpService::new(
        || Ok(OrchestrAiMcp::new()),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    )
}

/// Serve one MCP session over stdin/stdout until the client disconnects.
///
/// In stdio mode only protocol bytes may appear on stdout; logs must go
/// to stderr. Callers are responsible for not writing to stdout
/// themselves while this future is running.
pub async fn run_stdio() -> Result<(), Box<dyn std::error::Error>> {
    let service = OrchestrAiMcp::new().serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
