mod command;
use std::io::Write;
use std::process::{Command, ExitStatus, Output, Stdio};

use anyhow::{Result, bail};
use color_print::cprintln;

pub use crate::shell::command::ShellCommand;

/// Build a detailed failure message including exit code, stderr, and stdout.
/// Many tools (notably `swift build`) write diagnostics to stdout, so reporting
/// only stderr can leave the error empty.
fn format_failure(shell_cmd: &ShellCommand, output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let code = exit_code_str(&output.status);

    let mut msg = format!("{} failed (exit {code}):", shell_cmd.program);
    msg.push_str(&format!("\n  command: {shell_cmd}"));
    let stderr = stderr.trim();
    let stdout = stdout.trim();
    if !stderr.is_empty() {
        msg.push_str(&format!("\n--- stderr ---\n{stderr}"));
    }
    if !stdout.is_empty() {
        msg.push_str(&format!("\n--- stdout ---\n{stdout}"));
    }
    if stderr.is_empty() && stdout.is_empty() {
        msg.push_str("\n(no output captured on stdout or stderr)");
    }
    msg
}

fn exit_code_str(status: &ExitStatus) -> String {
    status
        .code()
        .map(|c| c.to_string())
        .unwrap_or_else(|| "signal".to_string())
}

pub struct Shell {
    pub dry_run: bool,
}

impl Shell {
    pub fn new(dry_run: bool) -> Self {
        Shell { dry_run }
    }

    /// Run a command, return trimmed stdout. Fails on non-zero exit.
    /// In dry-run, the env is printed for transparency.
    pub fn run<C: Into<ShellCommand>>(&self, shell_cmd: C) -> Result<String> {
        let shell_cmd = shell_cmd.into();
        shell_cmd.log(self.dry_run);
        if self.dry_run {
            return Ok(String::new());
        }
        let output = shell_cmd.command().output()?;
        if !output.status.success() {
            bail!(format_failure(&shell_cmd, &output));
        }
        Ok(String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_string())
    }
    /// Run a command whose arguments contain a secret (e.g. `security import
    /// -P`). Logs and reports `display` instead of the real args, so
    /// passwords never reach the terminal or an error message. Callers pass
    /// a redacted form.
    pub fn run_redacted(&self, args: &[&str], display: &str) -> Result<()> {
        if args.is_empty() {
            bail!("Empty command");
        }
        let prefix = if self.dry_run { "[dry-run] " } else { "" };
        cprintln!("<dim>{prefix}{display}</dim>");
        if self.dry_run {
            return Ok(());
        }
        let output = Command::new(args[0]).args(&args[1..]).output()?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let code = exit_code_str(&output.status);
            let mut msg = format!("{} failed (exit {code}):", args[0]);
            msg.push_str(&format!("\n  command: {display}"));
            let stderr = stderr.trim();
            if !stderr.is_empty() {
                msg.push_str(&format!("\n--- stderr ---\n{stderr}"));
            }
            bail!(msg);
        }
        Ok(())
    }

    /// Run a command with data piped to stdin. Fails on non-zero exit.
    pub fn run_stdin<C: Into<ShellCommand>>(
        &self,
        shell_cmd: C,
        stdin_data: &[u8],
    ) -> Result<String> {
        let shell_cmd = shell_cmd.into();
        shell_cmd.log_with_trailer(self.dry_run, &format!("<< <[{} bytes]>", stdin_data.len()));
        match std::str::from_utf8(stdin_data) {
            Ok(text) => cprintln!("<dim>{text}</dim>"),
            Err(_) => cprintln!("<dim>(binary data)</dim>",),
        }
        if self.dry_run {
            return Ok(String::new());
        }
        let mut child = shell_cmd
            .command()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        // Take and drop stdin handle after writing to signal EOF
        child.stdin.take().unwrap().write_all(stdin_data)?;
        let output = child.wait_with_output()?;
        if !output.status.success() {
            bail!(format_failure(&shell_cmd, &output));
        }
        Ok(String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_string())
    }

    /// Run a command, streaming stdout/stderr live to the terminal.
    /// Use for long-running commands (e.g. `swift build`) where progress
    /// should be visible. Output is not captured, so failures report only
    /// the exit code — the diagnostics are already on screen.
    pub fn run_streamed_env(&self, shell_cmd: ShellCommand) -> Result<()> {
        shell_cmd.log(self.dry_run);
        if self.dry_run {
            return Ok(());
        }
        let status = shell_cmd
            .command()
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()?;
        if !status.success() {
            let code = exit_code_str(&status);
            bail!(
                "{} failed (exit {code}):\n  command: {shell_cmd}\n  (output streamed above)",
                shell_cmd.program,
            );
        }
        Ok(())
    }
}
