use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use serde_json::{json, Value};
use crate::server::AppState;

pub async fn run_mcp_server(state: Arc<AppState>) -> anyhow::Result<()> {
    let stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let mut reader = BufReader::new(stdin);
    let mut line = String::new();

    loop {
        line.clear();
        if reader.read_line(&mut line).await? == 0 {
            break;
        }

        let request: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let method = request.get("method").and_then(|m| m.as_str()).unwrap_or("");

        let response = match method {
            "initialize" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {
                        "tools": {
                            "list_reports": {
                                "description": "List all discovered routes and their current Lighthouse scores",
                                "inputSchema": { "type": "object", "properties": {} }
                            },
                            "get_report_details": {
                                "description": "Get detailed audit results for a specific route ID",
                                "inputSchema": {
                                    "type": "object",
                                    "properties": {
                                        "id": { "type": "string", "description": "The route ID (usually a hashed URL)" }
                                    },
                                    "required": ["id"]
                                }
                            }
                        }
                    },
                    "serverInfo": { "name": "unlighthouse-rs", "version": "0.1.0" }
                }
            }),
            "tools/list" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "tools": [
                        {
                            "name": "list_reports",
                            "description": "List all discovered routes and their current Lighthouse scores",
                            "inputSchema": { "type": "object", "properties": {} }
                        },
                        {
                            "name": "get_report_details",
                            "description": "Get detailed audit results for a specific route ID",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "string", "description": "The route ID (usually a hashed URL)" }
                                },
                                "required": ["id"]
                            }
                        }
                    ]
                }
            }),
            "tools/call" => {
                let name = request.get("params").and_then(|p| p.get("name")).and_then(|n| n.as_str()).unwrap_or("");
                let arguments = request.get("params").and_then(|p| p.get("arguments")).cloned().unwrap_or(json!({}));
                
                match name {
                    "list_reports" => {
                        let reports = state.route_reports.read().await;
                        let data: Vec<Value> = reports.values().map(|r| json!({
                            "id": r.report_id,
                            "path": r.route.path,
                            "score": r.report.as_ref().map(|lh| lh.score * 100.0).unwrap_or(0.0),
                            "status": r.tasks.run_lighthouse_task
                        })).collect();
                        
                        json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": { "content": [{ "type": "text", "text": serde_json::to_string_pretty(&data).unwrap() }] }
                        })
                    },
                    "get_report_details" => {
                        let rid = arguments.get("id").and_then(|v| v.as_str()).unwrap_or("");
                        let reports = state.route_reports.read().await;
                        if let Some(r) = reports.get(rid) {
                            json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "result": { "content": [{ "type": "text", "text": serde_json::to_string_pretty(&r).unwrap() }] }
                            })
                        } else {
                            json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "error": { "code": -32602, "message": "Report not found" }
                            })
                        }
                    },
                    _ => json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": { "code": -32601, "message": "Tool not found" }
                    })
                }
            },
            _ => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": "Method not found" }
            }),
        };

        let response_str = serde_json::to_string(&response)? + "\n";
        stdout.write_all(response_str.as_bytes()).await?;
        stdout.flush().await?;
    }

    Ok(())
}
