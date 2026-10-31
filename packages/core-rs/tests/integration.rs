#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::Path;
    use std::process::{Command, Stdio};
    use std::thread;
    use std::time::{Duration, Instant};

    /// Spawn a minimal local HTTP server on an ephemeral port that serves a test HTML page,
    /// `/robots.txt`, and `/sitemap.xml` so CLI integration tests run offline in milliseconds.
    fn spawn_local_test_site() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind local test site");
        let addr = listener.local_addr().unwrap();
        let base_url = format!("http://{addr}");
        let base_clone = base_url.clone();

        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let base = base_clone.clone();
                thread::spawn(move || {
                    let mut stream = stream;
                    let mut buf = [0u8; 2048];
                    let n = stream.read(&mut buf).unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let first_line = req.lines().next().unwrap_or("");
                    let path = first_line.split_whitespace().nth(1).unwrap_or("/");

                    let (content_type, body) = match path {
                        "/robots.txt" => (
                            "text/plain",
                            format!("User-agent: *\nAllow: /\nSitemap: {base}/sitemap.xml\n"),
                        ),
                        "/sitemap.xml" => (
                            "application/xml",
                            format!(
                                r#"<?xml version="1.0" encoding="UTF-8"?>
                                <urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
                                    <url><loc>{base}/</loc></url>
                                </urlset>"#
                            ),
                        ),
                        _ => (
                            "text/html; charset=utf-8",
                            "<!DOCTYPE html><html><head><title>Local Test</title><meta name=\"description\" content=\"Desc\"></head><body><h1>Hello</h1></body></html>".to_string(),
                        ),
                    };

                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(resp.as_bytes());
                });
            }
        });

        base_url
    }

    /// Create a fast, deterministic mock Lighthouse worker script (executable bash/python)
    /// that supports both `--persistent` stdin/stdout pool protocol and one-off CLI invocations,
    /// writing `report.json`, `lighthouse.html`, `screenshot.jpeg`, and `full-screenshot.jpeg`.
    fn write_mock_lighthouse_script(dir: &Path) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let script_path = dir.join("mock_lighthouse_worker");
        fs::write(
            &script_path,
            r#"#!/usr/bin/env python3
import json
import os
import sys

def write_artifacts(output_dir):
    os.makedirs(output_dir, exist_ok=True)
    lhr = {
        "categories": {
            "performance": {"score": 0.92, "title": "Performance"},
            "accessibility": {"score": 0.96, "title": "Accessibility"},
            "best-practices": {"score": 0.94, "title": "Best Practices"},
            "seo": {"score": 0.98, "title": "SEO"}
        },
        "audits": {
            "first-contentful-paint": {"numericValue": 900}
        }
    }
    with open(os.path.join(output_dir, "report.json"), "w") as f:
        json.dump(lhr, f)
    with open(os.path.join(output_dir, "lighthouse.html"), "w") as f:
        f.write("<html><body>Report</body></html>")
    with open(os.path.join(output_dir, "screenshot.jpeg"), "wb") as f:
        f.write(b"\xff\xd8\xff\xd9")
    with open(os.path.join(output_dir, "full-screenshot.jpeg"), "wb") as f:
        f.write(b"\xff\xd8\xff\xd9")

args = sys.argv[1:]
if "--persistent" in args:
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        task = json.loads(line)
        write_artifacts(task["outputDir"])
        res = {
            "success": True,
            "url": task["url"],
            "scores": {"performance": 0.92, "accessibility": 0.96, "best-practices": 0.94, "seo": 0.98},
            "error": None
        }
        print("JSON_RESULT:" + json.dumps(res), flush=True)
else:
    if "--output-dir" in args:
        idx = args.index("--output-dir")
        if idx + 1 < len(args):
            write_artifacts(args[idx + 1])
"#,
        )
        .expect("write mock lighthouse script");

        let mut perms = fs::metadata(&script_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script_path, perms).unwrap();
        script_path
    }

    #[test]
    fn test_help_and_validation_guards() {
        let output = Command::new(env!("CARGO_BIN_EXE_unlighthouse-rs"))
            .arg("--help")
            .output()
            .expect("failed to execute process");

        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("unlighthouse-rs"));
        assert!(stdout.contains("--site"));

        // Missing --site should exit non-zero
        let no_site = Command::new(env!("CARGO_BIN_EXE_unlighthouse-rs"))
            .arg("--ci")
            .output()
            .expect("failed to execute process");
        assert!(!no_site.status.success());

        // Forbidden system output path (/etc/...) should be rejected
        let sys_path = Command::new(env!("CARGO_BIN_EXE_unlighthouse-rs"))
            .arg("--site")
            .arg("https://example.com")
            .arg("--output-path")
            .arg("/etc/unlighthouse-forbidden")
            .arg("--ci")
            .output()
            .expect("failed to execute process");
        assert!(!sys_path.status.success());
    }

    #[test]
    fn test_reporter_json_and_budget_enforcement() {
        let site_url = spawn_local_test_site();
        let temp = tempfile::tempdir().unwrap();
        let mock_lh = write_mock_lighthouse_script(temp.path());
        let output_dir = temp.path().join("out-json");

        // 1. Run with passing budget (80 <= 95)
        let output = Command::new(env!("CARGO_BIN_EXE_unlighthouse-rs"))
            .arg("--site")
            .arg(&site_url)
            .arg("--max-routes")
            .arg("1")
            .arg("--workers")
            .arg("1")
            .arg("--reporter")
            .arg("json")
            .arg("--budget")
            .arg("80")
            .arg("--output-path")
            .arg(&output_dir)
            .arg("--ci")
            .arg("--lighthouse-process-path")
            .arg(&mock_lh)
            .output()
            .expect("failed to execute process");

        assert!(
            output.status.success(),
            "Expected passing budget run to succeed, stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output_dir.join("report.json").exists());
        assert!(output_dir.join("unlighthouse.db").exists());

        // 2. Run with failing budget (99 > 95) -> must exit with code 1
        let fail_output = Command::new(env!("CARGO_BIN_EXE_unlighthouse-rs"))
            .arg("--site")
            .arg(&site_url)
            .arg("--max-routes")
            .arg("1")
            .arg("--workers")
            .arg("1")
            .arg("--budget")
            .arg("99")
            .arg("--output-path")
            .arg(temp.path().join("out-fail"))
            .arg("--ci")
            .arg("--lighthouse-process-path")
            .arg(&mock_lh)
            .output()
            .expect("failed to execute process");

        assert_eq!(fail_output.status.code(), Some(1));
    }

    #[test]
    fn test_screenshots_and_report_generation() {
        let site_url = spawn_local_test_site();
        let temp = tempfile::tempdir().unwrap();
        let mock_lh = write_mock_lighthouse_script(temp.path());
        let output_dir = temp.path().join("artifacts");

        let output = Command::new(env!("CARGO_BIN_EXE_unlighthouse-rs"))
            .arg("--site")
            .arg(&site_url)
            .arg("--max-routes")
            .arg("1")
            .arg("--workers")
            .arg("1")
            .arg("--output-path")
            .arg(&output_dir)
            .arg("--ci")
            .arg("--lighthouse-process-path")
            .arg(&mock_lh)
            .output()
            .expect("failed to execute process");

        assert!(
            output.status.success(),
            "CLI did not exit successfully: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let mut has_screenshot = false;
        let mut has_full_screenshot = false;
        let mut has_lighthouse_html = false;
        let mut has_report_json = false;

        fn visit_dirs(
            dir: &Path,
            has_screenshot: &mut bool,
            has_full_screenshot: &mut bool,
            has_lighthouse_html: &mut bool,
            has_report_json: &mut bool,
        ) -> std::io::Result<()> {
            if dir.is_dir() {
                for entry in fs::read_dir(dir)? {
                    let entry = entry?;
                    let path = entry.path();
                    if path.is_dir() {
                        visit_dirs(
                            &path,
                            has_screenshot,
                            has_full_screenshot,
                            has_lighthouse_html,
                            has_report_json,
                        )?;
                    } else if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
                        let size = fs::metadata(&path)?.len();
                        if file_name == "screenshot.jpeg" && size > 0 {
                            *has_screenshot = true;
                        } else if file_name == "full-screenshot.jpeg" && size > 0 {
                            *has_full_screenshot = true;
                        } else if file_name == "lighthouse.html" && size > 0 {
                            *has_lighthouse_html = true;
                        } else if file_name == "report.json" && size > 0 {
                            *has_report_json = true;
                        }
                    }
                }
            }
            Ok(())
        }

        visit_dirs(
            &output_dir,
            &mut has_screenshot,
            &mut has_full_screenshot,
            &mut has_lighthouse_html,
            &mut has_report_json,
        )
        .expect("failed to walk directories");

        assert!(has_screenshot, "screenshot.jpeg was not generated or is empty");
        assert!(has_full_screenshot, "full-screenshot.jpeg was not generated or is empty");
        assert!(has_lighthouse_html, "lighthouse.html was not generated or is empty");
        assert!(has_report_json, "report.json was not generated or is empty");
    }

    #[test]
    fn test_crux_history_proxy_and_server_endpoints() {
        let site_url = spawn_local_test_site();
        let temp = tempfile::tempdir().unwrap();
        let mock_lh = write_mock_lighthouse_script(temp.path());

        // Pick an available port
        let probe = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = probe.local_addr().unwrap().port();
        drop(probe);

        let mut child = Command::new(env!("CARGO_BIN_EXE_unlighthouse-rs"))
            .arg("--site")
            .arg(&site_url)
            .arg("--port")
            .arg(port.to_string())
            .arg("--workers")
            .arg("1")
            .arg("--output-path")
            .arg(temp.path().join("srv-out"))
            .arg("--lighthouse-process-path")
            .arg(&mock_lh)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn server process");

        // Poll until the HTTP server responds (typically < 300ms against local test site)
        let start = Instant::now();
        let mut stdout = String::new();
        while start.elapsed() < Duration::from_secs(10) {
            if let Ok(out) = Command::new("curl")
                .arg("-s")
                .arg(format!(
                    "http://localhost:{port}/api/crux/https%3A%2F%2Fwww.calljacob.com/history"
                ))
                .output()
            {
                let s = String::from_utf8_lossy(&out.stdout).to_string();
                if s.contains("\"exists\"") || s.contains("\"dates\"") {
                    stdout = s;
                    break;
                }
            }
            thread::sleep(Duration::from_millis(100));
        }

        let _ = child.kill();
        let _ = child.wait();

        assert!(
            stdout.contains("\"dates\"") || stdout.contains("\"exists\""),
            "CrUX proxy failed. stdout: {stdout}"
        );
    }

    #[test]
    fn test_per_route_budget_violation_and_port_fallback() {
        let site_url = spawn_local_test_site();
        let temp = tempfile::tempdir().unwrap();
        let mock_lh = write_mock_lighthouse_script(temp.path());

        // 1. Per-route [[budgets]] in config.toml where performance threshold (99) > actual (92)
        let cfg_path = temp.path().join("unlighthouse.config.toml");
        fs::write(
            &cfg_path,
            format!(
                r#"
site = "{site_url}"

[[budgets]]
path = "/**"
performance = 99.0
"#
            ),
        )
        .unwrap();

        let violation_out = Command::new(env!("CARGO_BIN_EXE_unlighthouse-rs"))
            .arg("-c")
            .arg(&cfg_path)
            .arg("--max-routes")
            .arg("1")
            .arg("--workers")
            .arg("1")
            .arg("--output-path")
            .arg(temp.path().join("budget-violation-out"))
            .arg("--ci")
            .arg("--lighthouse-process-path")
            .arg(&mock_lh)
            .output()
            .expect("failed to execute process");

        assert_eq!(
            violation_out.status.code(),
            Some(1),
            "Expected per-route budget violation to exit with code 1"
        );

        // 2. Port fallback when primary port is already occupied
        let occupied = TcpListener::bind("127.0.0.1:0").unwrap();
        let busy_port = occupied.local_addr().unwrap().port();
        let fallback_port = busy_port + 1;

        let mut child = Command::new(env!("CARGO_BIN_EXE_unlighthouse-rs"))
            .arg("--site")
            .arg(&site_url)
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--port")
            .arg(busy_port.to_string())
            .arg("--workers")
            .arg("1")
            .arg("--output-path")
            .arg(temp.path().join("fallback-out"))
            .arg("--lighthouse-process-path")
            .arg(&mock_lh)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn server process");

        let start = Instant::now();
        let mut meta_json = String::new();
        while start.elapsed() < Duration::from_secs(10) {
            if let Ok(out) = Command::new("curl")
                .arg("-s")
                .arg(format!("http://127.0.0.1:{fallback_port}/api/scan-meta"))
                .output()
            {
                let s = String::from_utf8_lossy(&out.stdout).to_string();
                if s.contains("\"routes\"") {
                    meta_json = s;
                    break;
                }
            }
            thread::sleep(Duration::from_millis(100));
        }

        let _ = child.kill();
        let _ = child.wait();
        drop(occupied);

        assert!(
            meta_json.contains("\"routes\""),
            "Expected server to bind fallback port {fallback_port}, got: {meta_json}"
        );
    }

    #[test]
    fn test_build_static_and_multi_sample_audit() {
        let site_url = spawn_local_test_site();
        let temp = tempfile::tempdir().unwrap();
        let mock_lh = write_mock_lighthouse_script(temp.path());
        let output_dir = temp.path().join("static-out");

        // Run with --build-static (without needing --ci) and --samples 2
        let out = Command::new(env!("CARGO_BIN_EXE_unlighthouse-rs"))
            .arg("--site")
            .arg(&site_url)
            .arg("--max-routes")
            .arg("2")
            .arg("--workers")
            .arg("1")
            .arg("--samples")
            .arg("2")
            .arg("--build-static")
            .arg("--output-path")
            .arg(&output_dir)
            .arg("--lighthouse-process-path")
            .arg(&mock_lh)
            .output()
            .expect("failed to execute process");

        assert!(
            out.status.success(),
            "CLI --build-static failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );

        let static_index = output_dir.join("client").join("index.html");
        assert!(static_index.exists(), "client/index.html was not generated by --build-static");
        let html = fs::read_to_string(&static_index).unwrap();
        assert!(html.contains("window.__unlighthouse_static = true;"));
        assert!(html.contains("window.__unlighthouse_payload ="));
        assert!(html.contains("\"reports\":"));
    }
}
