use std::collections::HashMap;
use std::fmt::Display;
use std::process::Command;

use color_print::{cformat, cprintln};

use crate::shell::Shell;

/// Builder for shell commands with support for dry run and env vars.
#[derive(Clone)]
pub struct ShellCommand {
    pub program: String,
    pub args: Vec<ShellArg>,
    pub env: HashMap<String, String>,
    pub hide_dry_run: bool,
}

impl ShellCommand {
    pub fn new(program: &str) -> Self {
        ShellCommand {
            program: program.to_string(),
            args: Vec::new(),
            env: HashMap::new(),
            hide_dry_run: false,
        }
    }

    // don't show this command in dry run mode
    pub fn hide_dry_run(mut self) -> Self {
        self.hide_dry_run = true;
        self
    }

    pub fn arg(mut self, arg: &str) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn args(mut self, args: &[&str]) -> Self {
        for arg in args {
            self.args.push((*arg).into());
        }
        self
    }

    pub fn arg_group(mut self, args: &[&str]) -> Self {
        let group = args.iter().map(|s| s.to_string()).collect();
        self.args.push(ShellArg::Group(group));
        self
    }

    pub fn envs(mut self, vars: &HashMap<String, String>) -> Self {
        for (k, v) in vars {
            self.env.insert(k.clone(), v.clone());
        }
        self
    }

    pub fn log(&self, dry_run: bool) {
        self.log_with_trailer(dry_run, "");
    }

    pub fn log_with_trailer(&self, dry_run: bool, trailer: &str) {
        let prefix = if dry_run { "[dry-run] " } else { "" };
        cprintln!("<dim>{prefix}{self} {trailer}</dim>");
    }

    pub fn command(&self) -> Command {
        let mut cmd = Command::new(&self.program);
        self.args
            .iter()
            .flat_map(|arg| Into::<Vec<String>>::into(arg.clone()))
            .for_each(|s| {
                cmd.arg(s);
            });

        cmd.envs(&self.env);
        cmd
    }

    pub fn run(self, shell: &Shell) -> Result<String, anyhow::Error> {
        shell.run(self)
    }
}

impl Display for ShellCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let env_str = self
            .env
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(" ");
        let env_prefix = if env_str.is_empty() {
            String::new()
        } else {
            format!("{env_str} ")
        };
        let cmd = self
            .args
            .iter()
            .map(|arg| match arg {
                ShellArg::Literal(s) => s.clone(),
                ShellArg::Group(group) => cformat!("<underline>{}</underline>", group.join(" ")),
            })
            .collect::<Vec<_>>()
            .join(" ");
        write!(f, "{env_prefix}{} {cmd}", self.program)
    }
}

impl From<&[&str]> for ShellCommand {
    fn from(args: &[&str]) -> ShellCommand {
        let cmd = ShellCommand::new(args[0]);
        cmd.args(&args[1..])
    }
}

impl<const N: usize> From<&[&str; N]> for ShellCommand {
    fn from(args: &[&str; N]) -> Self {
        Self::from(args.as_slice())
    }
}

#[derive(Clone)]
pub enum ShellArg {
    Literal(String),
    Group(Vec<String>),
}

impl From<&str> for ShellArg {
    fn from(val: &str) -> Self {
        ShellArg::Literal(val.to_string())
    }
}

impl From<ShellArg> for Vec<String> {
    fn from(val: ShellArg) -> Self {
        match val {
            ShellArg::Literal(s) => vec![s],
            ShellArg::Group(group) => group,
        }
    }
}
