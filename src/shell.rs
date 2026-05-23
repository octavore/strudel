use std::io::Write;
use std::process::{Command, Output, Stdio};

use anyhow::{Result, bail};
use color_print::cprintln;

/// Build a detailed failure message including exit code, stderr, and stdout.
/// Many tools (notably `swift build`) write diagnostics to stdout, so reporting
/// only stderr can leave the error empty.
fn format_failure(args: &[&str], output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let code = output
        .status
        .code()
        .map(|c| c.to_string())
        .unwrap_or_else(|| "signal".to_string());

    let mut msg = format!("{} failed (exit {}):", args[0], code);
    msg.push_str(&format!("\n  command: {}", args.join(" ")));
    let stderr = stderr.trim();
    let stdout = stdout.trim();
    if !stderr.is_empty() {
        msg.push_str(&format!("\n--- stderr ---\n{}", stderr));
    }
    if !stdout.is_empty() {
        msg.push_str(&format!("\n--- stdout ---\n{}", stdout));
    }
    if stderr.is_empty() && stdout.is_empty() {
        msg.push_str("\n(no output captured on stdout or stderr)");
    }
    msg
}

pub struct Shell {
    pub dry_run: bool,
}

impl Shell {
    pub fn new(dry_run: bool) -> Self {
        Shell { dry_run }
    }

    /// Run a command, return trimmed stdout. Fails on non-zero exit.
    pub fn run(&self, args: &[&str]) -> Result<String> {
        if args.is_empty() {
            bail!("Empty command");
        }
        if self.dry_run {
            cprintln!("<dim>[dry-run]</dim> {}", args.join(" "));
            return Ok(String::new());
        }
        let output = Command::new(args[0]).args(&args[1..]).output()?;
        if !output.status.success() {
            bail!(format_failure(args, &output));
        }
        Ok(String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_string())
    }

    /// Run a command, streaming stdout/stderr live to the terminal.
    /// Use for long-running commands (e.g. `swift build`) where progress
    /// should be visible. Output is not captured, so failures report only
    /// the exit code — the diagnostics are already on screen.
    pub fn run_streamed(&self, args: &[&str]) -> Result<()> {
        if args.is_empty() {
            bail!("Empty command");
        }
        if self.dry_run {
            cprintln!("<dim>[dry-run]</dim> {}", args.join(" "));
            return Ok(());
        }
        let status = Command::new(args[0])
            .args(&args[1..])
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()?;
        if !status.success() {
            let code = status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "signal".to_string());
            bail!(
                "{} failed (exit {}):\n  command: {}\n  (output streamed above)",
                args[0],
                code,
                args.join(" ")
            );
        }
        Ok(())
    }

    /// Run a command with data piped to stdin. Fails on non-zero exit.
    pub fn run_stdin(&self, args: &[&str], stdin_data: &[u8]) -> Result<String> {
        if args.is_empty() {
            bail!("Empty command");
        }
        if self.dry_run {
            cprintln!(
                "<dim>[dry-run]</dim> {} << <blue><<{} bytes>></blue>",
                args.join(" "),
                stdin_data.len()
            );
            return Ok(String::new());
        }
        let mut child = Command::new(args[0])
            .args(&args[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        // Take and drop stdin handle after writing to signal EOF
        child.stdin.take().unwrap().write_all(stdin_data)?;
        let output = child.wait_with_output()?;
        if !output.status.success() {
            bail!(format_failure(args, &output));
        }
        Ok(String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_string())
    }

    /// Run a command, ignoring failures (mirrors the spctl try/catch pattern).
    pub fn try_run(&self, args: &[&str]) {
        let _ = self.run(args);
    }
}
