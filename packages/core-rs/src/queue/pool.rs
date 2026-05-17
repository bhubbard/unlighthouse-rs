use std::process::Stdio;
use std::sync::Arc;
use anyhow::{Result, anyhow};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;
use tracing::info;
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
    _child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl PersistentWorker {
    pub async fn launch(config: &Config) -> Result<Self> {
        let mut cmd = Command::new("node");
        cmd.arg(&config.lighthouse_process_path)
            .arg("--persistent")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());

        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take().ok_or_else(|| anyhow!("Failed to open stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("Failed to open stdout"))?;
        let stdout = BufReader::new(stdout);

        Ok(Self { _child: child, stdin, stdout })
    }

    pub async fn audit(&mut self, task: AuditTask) -> Result<AuditResult> {
        let json = serde_json::to_string(&task)?;
        self.stdin.write_all(json.as_bytes()).await?;
        self.stdin.write_all(b"\n").await?;
        self.stdin.flush().await?;

        let mut line = String::new();
        self.stdout.read_line(&mut line).await?;
        
        if let Some(res_json) = line.strip_prefix("JSON_RESULT:") {
            let result: AuditResult = serde_json::from_str(res_json)?;
            Ok(result)
        } else {
            Err(anyhow!("Unexpected worker output: {}", line))
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

    pub async fn get_worker(&self, index: usize) -> Arc<Mutex<PersistentWorker>> {
        Arc::clone(&self.workers[index % self.workers.len()])
    }
}
