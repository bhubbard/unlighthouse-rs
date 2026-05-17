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
                            },
                            "list_runs": {
                                "description": "List all historical scan runs recorded in the database, showing site, start/finish times, and route count",
                                "inputSchema": {
                                    "type": "object",
                                    "properties": {
                                        "limit": { "type": "integer", "description": "Maximum number of runs to return (default: 10)" }
                                    }
                                }
                            },
                            "get_run_scores": {
                                "description": "Retrieve all route scores for a specific historical scan run",
                                "inputSchema": {
                                    "type": "object",
                                    "properties": {
                                        "runId": { "type": "string", "description": "The unique ID of the scan run" }
                                    },
                                    "required": ["runId"]
                                }
                            },
                            "get_route_history": {
                                "description": "Get the score trend for one specific route path across all finished runs (oldest-first for trending)",
                                "inputSchema": {
                                    "type": "object",
                                    "properties": {
                                        "path": { "type": "string", "description": "The URL path of the route (e.g., '/')" }
                                    },
                                    "required": ["path"]
                                }
                            },
                            "rescan_route": {
                                "description": "Trigger a fresh, high-priority rescan for a specific route path",
                                "inputSchema": {
                                    "type": "object",
                                    "properties": {
                                        "path": { "type": "string", "description": "The URL path of the route to rescan (e.g., '/about')" }
                                    },
                                    "required": ["path"]
                                }
                            },
                            "rescan_all": {
                                "description": "Clear all current reports and re-queue all routes for scanning",
                                "inputSchema": { "type": "object", "properties": {} }
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
                        },
                        {
                            "name": "list_runs",
                            "description": "List all historical scan runs recorded in the database, showing site, start/finish times, and route count",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "limit": { "type": "integer", "description": "Maximum number of runs to return (default: 10)" }
                                }
                            }
                        },
                        {
                            "name": "get_run_scores",
                            "description": "Retrieve all route scores for a specific historical scan run",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "runId": { "type": "string", "description": "The unique ID of the scan run" }
                                },
                                "required": ["runId"]
                            }
                        },
                        {
                            "name": "get_route_history",
                            "description": "Get the score trend for one specific route path across all finished runs (oldest-first for trending)",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "path": { "type": "string", "description": "The URL path of the route (e.g., '/')" }
                                },
                                "required": ["path"]
                            }
                        },
                        {
                            "name": "rescan_route",
                            "description": "Trigger a fresh, high-priority rescan for a specific route path",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "path": { "type": "string", "description": "The URL path of the route to rescan (e.g., '/about')" }
                                },
                                "required": ["path"]
                            }
                        },
                        {
                            "name": "rescan_all",
                            "description": "Clear all current reports and re-queue all routes for scanning",
                            "inputSchema": { "type": "object", "properties": {} }
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
                    "list_runs" => {
                        let limit = arguments.get("limit").and_then(|v| v.as_i64()).unwrap_or(10);
                        match crate::db::list_runs(&state.db, &state.config.site, limit).await {
                            Ok(runs) => json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "result": { "content": [{ "type": "text", "text": serde_json::to_string_pretty(&runs).unwrap() }] }
                            }),
                            Err(e) => json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "error": { "code": -32603, "message": format!("Database error: {}", e) }
                            })
                        }
                    },
                    "get_run_scores" => {
                        let run_id = arguments.get("runId").and_then(|v| v.as_str()).unwrap_or("");
                        match crate::db::get_run_scores(&state.db, run_id).await {
                            Ok(scores) => json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "result": { "content": [{ "type": "text", "text": serde_json::to_string_pretty(&scores).unwrap() }] }
                            }),
                            Err(e) => json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "error": { "code": -32603, "message": format!("Database error: {}", e) }
                            })
                        }
                    },
                    "get_route_history" => {
                        let path = arguments.get("path").and_then(|v| v.as_str()).unwrap_or("");
                        match crate::db::get_route_trend(&state.db, &state.config.site, path).await {
                            Ok(history) => json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "result": { "content": [{ "type": "text", "text": serde_json::to_string_pretty(&history).unwrap() }] }
                            }),
                            Err(e) => json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "error": { "code": -32603, "message": format!("Database error: {}", e) }
                            })
                        }
                    },
                    "rescan_route" => {
                        let path = arguments.get("path").and_then(|v| v.as_str()).unwrap_or("");
                        let mut reports = state.route_reports.write().await;
                        let mut found_id = None;
                        for (rid, report) in reports.iter() {
                            if report.route.path == path {
                                found_id = Some(rid.clone());
                                break;
                            }
                        }
                        if let Some(rid) = found_id {
                            if let Some(report) = reports.remove(&rid) {
                                let route = report.route.clone();
                                drop(reports);
                                let _ = state.work_tx.send(route).await;
                                json!({
                                    "jsonrpc": "2.0",
                                    "id": id,
                                    "result": { "content": [{ "type": "text", "text": format!("Successfully queued rescan for path: {}", path) }] }
                                })
                            } else {
                                json!({
                                    "jsonrpc": "2.0",
                                    "id": id,
                                    "error": { "code": -32602, "message": "Failed to remove route report" }
                                })
                            }
                        } else {
                            json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "error": { "code": -32602, "message": format!("Route path not found: {}", path) }
                            })
                        }
                    },
                    "rescan_all" => {
                        let mut reports = state.route_reports.write().await;
                        let count = reports.len();
                        let routes: Vec<_> = reports.values().map(|r| r.route.clone()).collect();
                        reports.clear();
                        drop(reports);
                        for route in routes {
                            let _ = state.work_tx.send(route).await;
                        }
                        json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": { "content": [{ "type": "text", "text": format!("Successfully cleared and queued all {} routes for rescan", count) }] }
                        })
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
