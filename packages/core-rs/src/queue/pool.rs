use std::process::Stdio;
use std::sync::Arc;
use anyhow::{Result, anyhow};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;
use tracing::{debug, info, warn};
use crate::config::Config;

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditTask {
    pub url: String,
    pub output_dir: String,
    pub device: String,
    pub throttle: bool,
    pub skip_javascript: bool,
    pub block_assets: bool,
    pub warmup: bool,
    pub auth: Option<crate::config::AuthConfig>,
    pub cookies: Option<Vec<crate::config::CookieConfig>>,
    pub local_storage: Option<std::collections::HashMap<String, serde_json::Value>>,
    pub session_storage: Option<std::collections::HashMap<String, serde_json::Value>>,
    pub extra_headers: Option<std::collections::HashMap<String, String>>,
    pub user_agent: Option<String>,
}

#[derive(serde::Deserialize)]
pub struct AuditResult {
    pub success: bool,
    pub url: String,
    pub scores: Option<std::collections::HashMap<String, f64>>,
    pub error: Option<String>,
}

pub struct PersistentWorker {
    process_path: String,
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Drop for PersistentWorker {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

impl PersistentWorker {
    fn spawn_child(process_path: &str) -> Result<(Child, ChildStdin, BufReader<ChildStdout>)> {
        let is_js = process_path.ends_with(".js")
            || process_path.ends_with(".mjs")
            || process_path.ends_with(".cjs");
        let mut cmd = if is_js {
            let mut c = Command::new("node");
            c.arg(process_path);
            c
        } else {
            Command::new(process_path)
        };
        cmd.arg("--persistent")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());

        let mut child = cmd.spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("Failed to open stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("Failed to open stdout"))?;
        Ok((child, stdin, BufReader::new(stdout)))
    }

    pub async fn launch(config: &Config) -> Result<Self> {
        let (child, stdin, stdout) = Self::spawn_child(&config.lighthouse_process_path)?;
        Ok(Self {
            process_path: config.lighthouse_process_path.clone(),
            child,
            stdin,
            stdout,
        })
    }

    async fn respawn(&mut self) -> Result<()> {
        let _ = self.child.start_kill();
        let _ = self.child.wait().await;
        let (child, stdin, stdout) = Self::spawn_child(&self.process_path)?;
        self.child = child;
        self.stdin = stdin;
        self.stdout = stdout;
        Ok(())
    }

    async fn try_audit_once(&mut self, payload: &[u8]) -> Result<AuditResult> {
        self.stdin.write_all(payload).await?;
        self.stdin.write_all(b"\n").await?;
        self.stdin.flush().await?;

        let mut line = String::new();
        loop {
            line.clear();
            let n = self.stdout.read_line(&mut line).await?;
            if n == 0 {
                return Err(anyhow!("Persistent worker exited unexpectedly (EOF)"));
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Some(res_json) = trimmed.strip_prefix("JSON_RESULT:") {
                let result: AuditResult = serde_json::from_str(res_json)?;
                return Ok(result);
            }
            debug!(output = %trimmed, "Ignoring non-protocol stdout line from Lighthouse worker");
        }
    }

    pub async fn audit(&mut self, task: AuditTask) -> Result<AuditResult> {
        let json = serde_json::to_string(&task)?;
        match self.try_audit_once(json.as_bytes()).await {
            Ok(res) => Ok(res),
            Err(first_err) => {
                warn!("Lighthouse persistent worker failed ({first_err}); respawning and retrying once...");
                self.respawn().await?;
                self.try_audit_once(json.as_bytes()).await
            }
        }
    }
}

pub struct LighthousePool {
    workers: Vec<Arc<Mutex<PersistentWorker>>>,
}

impl LighthousePool {
    pub async fn new(config: &Config, count: usize) -> Result<Self> {
        let mut workers = Vec::new();
        for i in 0..count {
            info!("Launching persistent Lighthouse worker #{}", i + 1);
            workers.push(Arc::new(Mutex::new(PersistentWorker::launch(config).await?)));
        }
        Ok(Self { workers })
    }

    pub async fn acquire_worker(&self) -> Arc<Mutex<PersistentWorker>> {
        loop {
            for worker in &self.workers {
                if let Ok(guard) = worker.try_lock() {
                    drop(guard);
                    return Arc::clone(worker);
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    pub fn active_workers(&self) -> usize {
        let mut count = 0;
        for worker in &self.workers {
            if worker.try_lock().is_err() {
                count += 1;
            }
        }
        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audit_task_and_result_serde() {
        let task = AuditTask {
            url: "https://example.com/".to_string(),
            output_dir: "/tmp/out".to_string(),
            device: "mobile".to_string(),
            throttle: true,
            skip_javascript: false,
            block_assets: true,
            warmup: false,
            auth: None,
            cookies: None,
            local_storage: None,
            session_storage: None,
            extra_headers: None,
            user_agent: Some("Bot/1.0".to_string()),
        };

        let val = serde_json::to_value(&task).unwrap();
        assert_eq!(val["outputDir"], "/tmp/out");
        assert_eq!(val["skipJavascript"], false);
        assert_eq!(val["blockAssets"], true);
        assert_eq!(val["userAgent"], "Bot/1.0");

        let res_json = r#"{"success":true,"url":"https://example.com/","scores":{"performance":0.91},"error":null}"#;
        let res: AuditResult = serde_json::from_str(res_json).unwrap();
        assert!(res.success);
        assert_eq!(res.scores.unwrap().get("performance"), Some(&0.91));
    }

    #[tokio::test]
    async fn test_lighthouse_pool_with_mock_worker() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let crash_marker = dir.path().join("crashed_once");
        let script_path = dir.path().join("mock_worker.sh");
        // Emits stray stdout noise before JSON_RESULT, and simulates a crash on first launch if marker doesn't exist
        std::fs::write(
            &script_path,
            format!(
                r#"#!/usr/bin/env bash
if [ ! -f "{marker}" ]; then
    touch "{marker}"
    exit 1
fi
while IFS= read -r line; do
    [ -z "$line" ] && continue
    echo "Chrome DevTools listening on ws://127.0.0.1:9222"
    echo 'JSON_RESULT:{{"success":true,"url":"https://example.com/test","scores":{{"performance":0.96,"seo":1.0}},"error":null}}'
done
"#,
                marker = crash_marker.display()
            ),
        )
        .unwrap();
        let mut perms = std::fs::metadata(&script_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script_path, perms).unwrap();

        let config = Config {
            lighthouse_process_path: script_path.to_str().unwrap().to_string(),
            ..Default::default()
        };

        let pool = LighthousePool::new(&config, 1).await.unwrap();
        assert_eq!(pool.active_workers(), 0);

        let worker_arc = pool.acquire_worker().await;
        let mut guard = worker_arc.lock().await;
        assert_eq!(pool.active_workers(), 1);

        let res = guard
            .audit(AuditTask {
                url: "https://example.com/test".to_string(),
                output_dir: dir.path().to_str().unwrap().to_string(),
                device: "desktop".to_string(),
                throttle: false,
                skip_javascript: false,
                block_assets: false,
                warmup: false,
                auth: None,
                cookies: None,
                local_storage: None,
                session_storage: None,
                extra_headers: None,
                user_agent: None,
            })
            .await
            .unwrap();

        drop(guard);
        assert_eq!(pool.active_workers(), 0);
        assert!(res.success);
        assert_eq!(res.url, "https://example.com/test");
        assert_eq!(res.scores.unwrap().get("performance"), Some(&0.96));
    }
}
