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

        let response = handle_mcp_request(&state, &request).await;

        let response_str = serde_json::to_string(&response)? + "\n";
        stdout.write_all(response_str.as_bytes()).await?;
        stdout.flush().await?;
    }

    Ok(())
}

/// Process a single MCP JSON-RPC request against `AppState` and return the JSON-RPC response.
pub async fn handle_mcp_request(state: &Arc<AppState>, request: &Value) -> Value {
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let method = request.get("method").and_then(|m| m.as_str()).unwrap_or("");

    match method {
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
                            },
                            "audit_url": {
                                "description": "Dynamically enqueue a new or existing route path or same-origin URL for auditing",
                                "inputSchema": {
                                    "type": "object",
                                    "properties": {
                                        "url": { "type": "string", "description": "Route path (e.g. '/pricing') or full same-origin URL to audit" }
                                    },
                                    "required": ["url"]
                                }
                            },
                            "check_budgets": {
                                "description": "Evaluate global and per-route score budgets against current scan reports",
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
                        },
                        {
                            "name": "audit_url",
                            "description": "Dynamically enqueue a new or existing route path or same-origin URL for auditing",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "url": { "type": "string", "description": "Route path (e.g. '/pricing') or full same-origin URL to audit" }
                                },
                                "required": ["url"]
                            }
                        },
                        {
                            "name": "check_budgets",
                            "description": "Evaluate global and per-route score budgets against current scan reports",
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
                                let artifact_path = report.artifact_path.clone();
                                drop(reports);
                                let cached_report = std::path::PathBuf::from(&artifact_path).join("report.json");
                                let _ = tokio::fs::remove_file(cached_report).await;
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
                        let items: Vec<_> = reports
                            .values()
                            .map(|r| (r.route.clone(), r.artifact_path.clone()))
                            .collect();
                        reports.clear();
                        drop(reports);
                        for (route, artifact_path) in items {
                            let cached_report = std::path::PathBuf::from(&artifact_path).join("report.json");
                            let _ = tokio::fs::remove_file(cached_report).await;
                            let _ = state.work_tx.send(route).await;
                        }
                        json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": { "content": [{ "type": "text", "text": format!("Successfully cleared and queued all {} routes for rescan", count) }] }
                        })
                    },
                    "audit_url" => {
                        let target = arguments
                            .get("url")
                            .or_else(|| arguments.get("path"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        match super::api::enqueue_route_target(state, target).await {
                            Ok(route) => json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "result": { "content": [{ "type": "text", "text": serde_json::to_string_pretty(&route).unwrap() }] }
                            }),
                            Err(msg) => json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "error": { "code": -32602, "message": msg }
                            }),
                        }
                    },
                    "check_budgets" => {
                        let result = super::api::evaluate_budgets(state).await;
                        json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": { "content": [{ "type": "text", "text": serde_json::to_string_pretty(&result).unwrap() }] }
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
        }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::types::{
        LighthouseCategoryScore, LighthouseReport, NormalisedRoute, RouteDefinition, RouteReport,
        TaskMap, TaskStatus,
    };
    use std::collections::HashMap;

    #[tokio::test]
    async fn test_mcp_all_jsonrpc_methods_and_tools() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("mcp.db");
        let db = crate::db::open(db_path.to_str().unwrap()).await.unwrap();
        crate::db::insert_run(&db, "run-mcp", "https://example.com", "full")
            .await
            .unwrap();

        let config = Arc::new(Config {
            site: "https://example.com".to_string(),
            output_path: dir.path().to_str().unwrap().to_string(),
            ..Default::default()
        });
        let (work_tx, mut work_rx) = tokio::sync::mpsc::channel(16);
        let state = Arc::new(AppState::new(
            config,
            work_tx,
            None,
            db,
            "run-mcp".to_string(),
        ));

        let mut categories = HashMap::new();
        categories.insert(
            "performance".to_string(),
            LighthouseCategoryScore {
                score: Some(0.95),
                title: None,
                key: None,
            },
        );
        let rep = RouteReport {
            report_id: "r1".to_string(),
            route: NormalisedRoute {
                id: "r1".to_string(),
                path: "/about".to_string(),
                url: "https://example.com/about".to_string(),
                definition: RouteDefinition {
                    name: "/about".to_string(),
                    path: "/about".to_string(),
                },
            },
            artifact_path: "/tmp/r1".to_string(),
            artifact_url: "/r1/".to_string(),
            tasks: TaskMap {
                inspect_html_task: TaskStatus::Completed,
                run_lighthouse_task: TaskStatus::Completed,
            },
            report: Some(LighthouseReport {
                score: 0.95,
                categories,
                audits: serde_json::Value::Null,
            }),
            seo: None,
            web_vitals: None,
        };

        state.route_reports.write().await.insert("r1".to_string(), rep.clone());
        crate::db::insert_route_score(&state.db, "run-mcp", &rep)
            .await
            .unwrap();
        crate::db::finish_run(&state.db, "run-mcp", 1).await.unwrap();

        // 1. initialize
        let init_resp = handle_mcp_request(
            &state,
            &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"}),
        )
        .await;
        assert_eq!(init_resp["result"]["serverInfo"]["name"], "unlighthouse-rs");

        // 2. tools/list
        let list_resp = handle_mcp_request(
            &state,
            &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
        )
        .await;
        assert_eq!(list_resp["result"]["tools"].as_array().unwrap().len(), 9);

        // 3. tools/call -> list_reports
        let lr_resp = handle_mcp_request(
            &state,
            &json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": { "name": "list_reports", "arguments": {} }
            }),
        )
        .await;
        assert!(lr_resp["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("/about"));

        // 4. tools/call -> get_report_details (found & not found)
        let det_ok = handle_mcp_request(
            &state,
            &json!({
                "jsonrpc": "2.0",
                "id": 4,
                "method": "tools/call",
                "params": { "name": "get_report_details", "arguments": { "id": "r1" } }
            }),
        )
        .await;
        assert!(det_ok["result"].is_object());

        let det_err = handle_mcp_request(
            &state,
            &json!({
                "jsonrpc": "2.0",
                "id": 5,
                "method": "tools/call",
                "params": { "name": "get_report_details", "arguments": { "id": "missing" } }
            }),
        )
        .await;
        assert_eq!(det_err["error"]["code"], -32602);

        // 5. tools/call -> list_runs, get_run_scores, get_route_history
        let runs_resp = handle_mcp_request(
            &state,
            &json!({
                "jsonrpc": "2.0",
                "id": 6,
                "method": "tools/call",
                "params": { "name": "list_runs", "arguments": { "limit": 5 } }
            }),
        )
        .await;
        assert!(runs_resp["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("run-mcp"));

        let scores_resp = handle_mcp_request(
            &state,
            &json!({
                "jsonrpc": "2.0",
                "id": 7,
                "method": "tools/call",
                "params": { "name": "get_run_scores", "arguments": { "runId": "run-mcp" } }
            }),
        )
        .await;
        assert!(scores_resp["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("/about"));

        let hist_resp = handle_mcp_request(
            &state,
            &json!({
                "jsonrpc": "2.0",
                "id": 8,
                "method": "tools/call",
                "params": { "name": "get_route_history", "arguments": { "path": "/about" } }
            }),
        )
        .await;
        assert!(hist_resp["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("run-mcp"));

        // 6. tools/call -> rescan_route & rescan_all
        let rescan_resp = handle_mcp_request(
            &state,
            &json!({
                "jsonrpc": "2.0",
                "id": 9,
                "method": "tools/call",
                "params": { "name": "rescan_route", "arguments": { "path": "/about" } }
            }),
        )
        .await;
        assert!(rescan_resp["result"].is_object());
        assert_eq!(work_rx.try_recv().unwrap().path, "/about");

        // Re-insert and test rescan_all
        state.route_reports.write().await.insert("r1".to_string(), rep);
        let rescan_all_resp = handle_mcp_request(
            &state,
            &json!({
                "jsonrpc": "2.0",
                "id": 10,
                "method": "tools/call",
                "params": { "name": "rescan_all", "arguments": {} }
            }),
        )
        .await;
        assert!(rescan_all_resp["result"].is_object());
        assert_eq!(work_rx.try_recv().unwrap().path, "/about");

        // 6.5 tools/call -> audit_url & check_budgets
        let audit_ok = handle_mcp_request(
            &state,
            &json!({
                "jsonrpc": "2.0",
                "id": 100,
                "method": "tools/call",
                "params": { "name": "audit_url", "arguments": { "url": "/pricing" } }
            }),
        )
        .await;
        assert!(audit_ok["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("/pricing"));
        assert_eq!(work_rx.try_recv().unwrap().path, "/pricing");

        let audit_cross_origin = handle_mcp_request(
            &state,
            &json!({
                "jsonrpc": "2.0",
                "id": 101,
                "method": "tools/call",
                "params": { "name": "audit_url", "arguments": { "url": "https://evil.org/phish" } }
            }),
        )
        .await;
        assert_eq!(audit_cross_origin["error"]["code"], -32602);

        let budget_resp = handle_mcp_request(
            &state,
            &json!({
                "jsonrpc": "2.0",
                "id": 102,
                "method": "tools/call",
                "params": { "name": "check_budgets", "arguments": {} }
            }),
        )
        .await;
        assert!(budget_resp["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("\"passed\": true"));

        // 7. Unknown tool & unknown method
        let unk_tool = handle_mcp_request(
            &state,
            &json!({
                "jsonrpc": "2.0",
                "id": 11,
                "method": "tools/call",
                "params": { "name": "nonexistent_tool" }
            }),
        )
        .await;
        assert_eq!(unk_tool["error"]["code"], -32601);

        let unk_method = handle_mcp_request(
            &state,
            &json!({
                "jsonrpc": "2.0",
                "id": 12,
                "method": "unknown/method"
            }),
        )
        .await;
        assert_eq!(unk_method["error"]["code"], -32601);
    }
}
