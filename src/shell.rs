use std::io::Write;
use std::process::{Command, Stdio};

use anyhow::{Result, bail};
use color_print::cprintln;

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
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("{} failed:\n{}", args[0], stderr.trim());
        }
        Ok(String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_string())
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
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("{} failed:\n{}", args[0], stderr.trim());
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
