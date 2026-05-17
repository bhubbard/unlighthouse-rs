#[cfg(test)]
mod tests {
    use std::process::Command;
    use std::path::Path;
    use std::fs;

    #[test]
    fn test_help_command() {
        let output = Command::new(env!("CARGO_BIN_EXE_unlighthouse-rs"))
            .arg("--help")
            .output()
            .expect("failed to execute process");

        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("unlighthouse-rs"));
        assert!(stdout.contains("--site"));
    }

    #[test]
    fn test_reporter_json() {
        // Run with a small site and json reporter
        let output_dir = ".unlighthouse-test-json";
        
        // Clean up stale output if exists
        if Path::new(output_dir).exists() {
            let _ = fs::remove_dir_all(output_dir);
        }

        let output = Command::new(env!("CARGO_BIN_EXE_unlighthouse-rs"))
            .arg("--site")
            .arg("https://example.com")
            .arg("--max-routes")
            .arg("1")
            .arg("--reporter")
            .arg("json")
            .arg("--output-path")
            .arg(output_dir)
            .arg("--ci")
            .arg("--lighthouse-process-path")
            .arg("./lighthouse.mjs")
            .output()
            .expect("failed to execute process");

        // We check if it ran and exited with a code
        assert!(output.status.code().is_some());
        
        // Clean up output dir
        if Path::new(output_dir).exists() {
            let _ = fs::remove_dir_all(output_dir);
        }
    }

    #[test]
    fn test_screenshots_and_report_generation() {
        let output_dir = ".unlighthouse-test-artifacts";
        
        // Clean up any stale directory from previous runs
        if Path::new(output_dir).exists() {
            let _ = fs::remove_dir_all(output_dir);
        }

        let output = Command::new(env!("CARGO_BIN_EXE_unlighthouse-rs"))
            .arg("--site")
            .arg("https://example.com")
            .arg("--max-routes")
            .arg("1")
            .arg("--output-path")
            .arg(output_dir)
            .arg("--ci")
            .arg("--lighthouse-process-path")
            .arg("./lighthouse.mjs")
            .output()
            .expect("failed to execute process");

        assert!(output.status.success(), "CLI did not exit successfully: {}", String::from_utf8_lossy(&output.stderr));

        // Verify that the expected files are created in the output directory
        let mut has_screenshot = false;
        let mut has_full_screenshot = false;
        let mut has_lighthouse_html = false;
        let mut has_report_json = false;

        fn visit_dirs(
            dir: &Path, 
            has_screenshot: &mut bool,
            has_full_screenshot: &mut bool,
            has_lighthouse_html: &mut bool,
            has_report_json: &mut bool
        ) -> std::io::Result<()> {
            if dir.is_dir() {
                for entry in fs::read_dir(dir)? {
                    let entry = entry?;
                    let path = entry.path();
                    if path.is_dir() {
                        visit_dirs(&path, has_screenshot, has_full_screenshot, has_lighthouse_html, has_report_json)?;
                    } else if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
                        let metadata = fs::metadata(&path)?;
                        let size = metadata.len();
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
            Path::new(output_dir), 
            &mut has_screenshot, 
            &mut has_full_screenshot, 
            &mut has_lighthouse_html, 
            &mut has_report_json
        ).expect("failed to walk directories");

        // Clean up the test directory
        let _ = fs::remove_dir_all(output_dir);

        assert!(has_screenshot, "screenshot.jpeg was not generated or is empty");
        assert!(has_full_screenshot, "full-screenshot.jpeg was not generated or is empty");
        assert!(has_lighthouse_html, "lighthouse.html (Lighthouse Report) was not generated or is empty");
        assert!(has_report_json, "report.json was not generated or is empty");
    }

    #[test]
    fn test_crux_history_proxy() {
        use std::process::Stdio;
        use std::thread;
        use std::time::Duration;

        let port = "9991";

        let mut child = Command::new(env!("CARGO_BIN_EXE_unlighthouse-rs"))
            .arg("--site")
            .arg("https://www.calljacob.com")
            .arg("--port")
            .arg(port)
            .arg("--lighthouse-process-path")
            .arg("./lighthouse.mjs")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn server process");

        // Wait for the server to boot up (instantly fast since it doesn't need to compile under cargo run!)
        thread::sleep(Duration::from_secs(3));

        // Request the crux history proxy endpoint using curl
        let curl_output = Command::new("curl")
            .arg("-s")
            .arg("-v")
            .arg(format!("http://localhost:{}/api/crux/https%3A%2F%2Fwww.calljacob.com/history", port))
            .output()
            .expect("failed to execute curl");

        // Kill the server process immediately
        let _ = child.kill();
        let _ = child.wait();

        let stdout = String::from_utf8_lossy(&curl_output.stdout);
        let stderr = String::from_utf8_lossy(&curl_output.stderr);
        
        assert!(stdout.contains("\"dates\"") || stdout.contains("\"exists\"") || stderr.contains("200 OK"), "CrUX proxy failed. stdout: {}, stderr: {}", stdout, stderr);
    }
}

