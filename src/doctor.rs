use std::net::{TcpStream, ToSocketAddrs};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

const DOCKER_REGISTRY_HOST: &str = "registry-1.docker.io";
const DOCKER_REGISTRY_PORT: u16 = 443;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckStatus {
    Pass,
    Fail,
    Warn,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckResult {
    pub name: &'static str,
    pub status: CheckStatus,
    pub message: String,
}

#[allow(clippy::module_name_repetitions)]
pub fn run_doctor() -> i32 {
    let results = collect_check_results();
    render_status_table(&results);
    i32::from(
        results
            .iter()
            .any(|result| result.status == CheckStatus::Fail),
    )
}

pub fn collect_check_results() -> Vec<CheckResult> {
    vec![
        check_docker_running(),
        check_claude_present(),
        check_claude_authenticated(),
        check_python_version(),
        check_swebench_version(),
        check_harness_cli(),
        check_git_present(),
        check_docker_registry_reachable(),
        check_swebench_docker_image(),
    ]
}

fn check_docker_running() -> CheckResult {
    match run_command_with_timeout(&["docker", "info"], Duration::from_secs(10)) {
        Ok(output) if output.status.success() => pass("Docker running", "docker info succeeded"),
        _ => fail(
            "Docker running",
            "Docker is not running. Start Docker Desktop or the Docker daemon.",
        ),
    }
}

fn check_claude_present() -> CheckResult {
    match run_command_with_timeout(&["claude", "--version"], Duration::from_secs(5)) {
        Ok(output)
            if output.status.success()
                && !stdout_text(&output).trim().is_empty() =>
        {
            pass("Claude CLI present", stdout_text(&output).trim())
        }
        _ => fail(
            "Claude CLI present",
            "Claude CLI not found. Install from https://claude.ai/download and ensure it is on PATH.",
        ),
    }
}

fn check_claude_authenticated() -> CheckResult {
    match run_command_with_timeout(
        &["claude", "-p", "--output-format", "text", "ping"],
        Duration::from_secs(15),
    ) {
        Ok(output) if output.status.success() && !stdout_text(&output).trim().is_empty() => {
            pass("Claude authenticated", "claude ping succeeded")
        }
        _ => fail(
            "Claude authenticated",
            "Claude CLI is not authenticated. Run `claude` interactively to log in.",
        ),
    }
}

fn check_python_version() -> CheckResult {
    match Command::new("python3").arg("--version").output() {
        Ok(output) if output.status.success() => {
            let version_text = stdout_text(&output);
            match parse_python_version(&version_text) {
                Some((major, minor)) if python_version_ok(major, minor) => {
                    pass("Python 3.11+", version_text.trim())
                }
                Some((major, minor)) => fail(
                    "Python 3.11+",
                    &format!("Python 3.11+ required. Found: {major}.{minor}."),
                ),
                None => fail(
                    "Python 3.11+",
                    &format!("Python 3.11+ required. Found: {}.", version_text.trim()),
                ),
            }
        }
        _ => fail("Python 3.11+", "Python 3.11+ required. Found: unavailable."),
    }
}

fn check_swebench_version() -> CheckResult {
    match Command::new("python3")
        .args(["-c", "import swebench; print(swebench.__version__)"])
        .output()
    {
        Ok(output) if output.status.success() && !stdout_text(&output).trim().is_empty() => {
            pass("swebench", stdout_text(&output).trim())
        }
        _ => fail(
            "swebench",
            "swebench required. Install: python3 -m pip install --upgrade swebench",
        ),
    }
}

fn check_harness_cli() -> CheckResult {
    match run_command_with_timeout(
        &["python3", "-m", "swebench.harness.run_evaluation", "--help"],
        Duration::from_secs(10),
    ) {
        Ok(output) if output.status.success() => {
            pass("SWE-bench harness CLI", "harness --help succeeded")
        }
        _ => fail(
            "SWE-bench harness CLI",
            "SWE-bench harness CLI not reachable. Reinstall swebench.",
        ),
    }
}

fn check_git_present() -> CheckResult {
    match Command::new("git").arg("--version").output() {
        Ok(output) if output.status.success() => pass("git", stdout_text(&output).trim()),
        _ => fail("git", "git not found. Install git."),
    }
}

fn check_docker_registry_reachable() -> CheckResult {
    match registry_reachable(Duration::from_secs(5)) {
        Ok(()) => pass(
            "Docker registry reachable",
            "registry-1.docker.io:443 reachable",
        ),
        Err(reason) => CheckResult {
            name: "Docker registry reachable",
            status: CheckStatus::Warn,
            message: format!(
                "Cannot reach registry-1.docker.io:443 ({reason}). \
                 The harness pulls SWE-bench images from Docker Hub; if Docker's \
                 own DNS is broken (e.g. `lookup registry-1.docker.io: no such host`) \
                 runs will fail. Check your network/VPN and Docker Desktop DNS settings."
            ),
        },
    }
}

fn registry_reachable(timeout: Duration) -> Result<(), String> {
    let addr = (DOCKER_REGISTRY_HOST, DOCKER_REGISTRY_PORT)
        .to_socket_addrs()
        .map_err(|e| format!("DNS lookup failed: {e}"))?
        .next()
        .ok_or_else(|| "DNS lookup returned no addresses".to_string())?;
    TcpStream::connect_timeout(&addr, timeout)
        .map(|_| ())
        .map_err(|e| format!("TCP connect failed: {e}"))
}

fn check_swebench_docker_image() -> CheckResult {
    match Command::new("docker")
        .args(["images", "-q", "swebench/sweb.eval.x86_64"])
        .output()
    {
        Ok(output) if output.status.success() && !stdout_text(&output).trim().is_empty() => {
            pass("SWE-bench Docker image", "image present")
        }
        _ => CheckResult {
            name: "SWE-bench Docker image",
            status: CheckStatus::Warn,
            message: "SWE-bench Docker image not pulled. First run will pull it (may take time)."
                .to_string(),
        },
    }
}

fn run_command_with_timeout(argv: &[&str], timeout: Duration) -> Result<Output, String> {
    let program = argv
        .first()
        .ok_or_else(|| "command argv cannot be empty".to_string())?;
    let mut child = Command::new(program)
        .args(&argv[1..])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to spawn {program}: {e}"))?;

    let started = Instant::now();
    loop {
        if child
            .try_wait()
            .map_err(|e| format!("failed to wait on {program}: {e}"))?
            .is_some()
        {
            return child
                .wait_with_output()
                .map_err(|e| format!("failed to collect output from {program}: {e}"));
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("command timed out after {}s", timeout.as_secs()));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn stdout_text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub fn parse_python_version(text: &str) -> Option<(u32, u32)> {
    let token = text
        .split_whitespace()
        .find(|part| part.chars().next().is_some_and(|ch| ch == '3'))?;
    let numbers: Vec<&str> = token.split('.').collect();
    if numbers.len() < 2 {
        return None;
    }
    let major = numbers[0].parse().ok()?;
    let minor = numbers[1]
        .trim_end_matches(|c: char| !c.is_ascii_digit())
        .parse()
        .ok()?;
    Some((major, minor))
}

pub fn python_version_ok(major: u32, minor: u32) -> bool {
    major > 3 || (major == 3 && minor >= 11)
}

fn pass(name: &'static str, message: &str) -> CheckResult {
    CheckResult {
        name,
        status: CheckStatus::Pass,
        message: message.to_string(),
    }
}

fn fail(name: &'static str, message: &str) -> CheckResult {
    CheckResult {
        name,
        status: CheckStatus::Fail,
        message: message.to_string(),
    }
}

fn render_status_table(results: &[CheckResult]) {
    println!("clawmark doctor");
    println!("---------------");
    for result in results {
        let status = match result.status {
            CheckStatus::Pass => "PASS",
            CheckStatus::Fail => "FAIL",
            CheckStatus::Warn => "WARN",
        };
        println!("{:<25} {status:<4} {}", result.name, result.message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_python_version_accepts_311() {
        assert_eq!(parse_python_version("Python 3.11.8"), Some((3, 11)));
    }

    #[test]
    fn parse_python_version_rejects_310() {
        assert_eq!(parse_python_version("Python 3.10.9"), Some((3, 10)));
        assert!(!python_version_ok(3, 10));
    }

    #[test]
    fn python_version_ok_accepts_312() {
        assert!(python_version_ok(3, 12));
    }

    #[test]
    fn doctor_exit_code_fails_on_failed_check() {
        let results = [
            CheckResult {
                name: "ok",
                status: CheckStatus::Pass,
                message: "ok".to_string(),
            },
            CheckResult {
                name: "bad",
                status: CheckStatus::Fail,
                message: "bad".to_string(),
            },
        ];
        assert!(results.iter().any(|r| r.status == CheckStatus::Fail));
    }

    #[test]
    fn doctor_warn_does_not_count_as_failure_logic() {
        let results = [CheckResult {
            name: "warn",
            status: CheckStatus::Warn,
            message: "warn".to_string(),
        }];
        assert!(!results
            .iter()
            .any(|result| result.status == CheckStatus::Fail));
    }
}
