use anyhow::Result;

use crate::config;

/// Print the MCP client configuration JSON for editors (Claude Desktop, Cursor, Zed, etc.)
pub fn cmd_mcp_show() -> Result<()> {
    let cfg = config::load()?;
    let exe = std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "transcriptd".to_string());

    // Stdio transport config (for Claude Desktop / Cursor)
    let stdio_config = serde_json::json!({
        "mcpServers": {
            "transcriptd": {
                "command": exe,
                "args": ["mcp", "stdio"]
            }
        }
    });

    println!("# Stdio transport (Claude Desktop / Cursor / Zed)");
    println!("# Add this to your editor's MCP configuration:\n");
    println!("{}\n", serde_json::to_string_pretty(&stdio_config)?);

    // HTTP transport config (for programmatic access)
    let http_config = serde_json::json!({
        "mcpServers": {
            "transcriptd": {
                "url": format!("http://127.0.0.1:{}/mcp", cfg.mcp_port),
                "headers": {
                    "Authorization": format!("Bearer {}", cfg.api_key)
                }
            }
        }
    });

    println!("# HTTP transport (start with: transcriptd mcp serve)\n");
    println!("{}", serde_json::to_string_pretty(&http_config)?);

    Ok(())
}
