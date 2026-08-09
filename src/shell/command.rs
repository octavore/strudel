use std::collections::HashMap;
use std::fmt::Display;
use std::path::PathBuf;
use std::process::Command;

use clml::{cformat, cprintln};
use secrecy::{ExposeSecret, SecretString};

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

    pub fn arg<A>(mut self, arg: A) -> Self
    where
        A: Into<ShellArg>,
    {
        self.args.push(arg.into());
        self
    }

    pub fn args<I>(mut self, args: I) -> Self
    where
        I: IntoIterator,
        I::Item: Into<ShellArg>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn arg_with_secret<K>(mut self, key: K, value: SecretString) -> Self
    where
        K: ToString,
    {
        self.args.push(ShellArg::SecretPair(key.to_string(), value));
        self
    }

    pub fn arg_group<A>(mut self, args: A) -> Self
    where
        A: IntoIterator,
        A::Item: ToString,
    {
        let group = args.into_iter().map(|s| s.to_string()).collect();
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
                ShellArg::Secret(_) => "<redacted>".into(),
                ShellArg::SecretPair(s, _) => cformat!("<underline>{s} <<redacted>></underline>"),
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
        cmd.args(args[1..].iter().copied())
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
    Secret(SecretString),
    SecretPair(String, SecretString),
    Group(Vec<String>),
}

impl From<&str> for ShellArg {
    fn from(val: &str) -> Self {
        ShellArg::Literal(val.to_string())
    }
}

impl From<String> for ShellArg {
    fn from(val: String) -> Self {
        ShellArg::Literal(val)
    }
}

impl From<&String> for ShellArg {
    fn from(val: &String) -> Self {
        ShellArg::Literal(val.clone())
    }
}

impl From<PathBuf> for ShellArg {
    fn from(val: PathBuf) -> Self {
        ShellArg::Literal(val.to_string_lossy().into_owned())
    }
}

impl From<SecretString> for ShellArg {
    fn from(val: SecretString) -> Self {
        ShellArg::Secret(val)
    }
}

impl From<ShellArg> for Vec<String> {
    fn from(val: ShellArg) -> Self {
        match val {
            ShellArg::Literal(s) => vec![s],
            ShellArg::Secret(s) => vec![s.expose_secret().to_owned()],
            ShellArg::SecretPair(s, secret) => {
                vec![s, secret.expose_secret().to_owned()]
            },
            ShellArg::Group(group) => group,
        }
    }
}

#[cfg(test)]
mod tests {
    use anstream::adapter::strip_str;

    use super::*;

    #[test]
    fn display_renders_program_and_args() {
        let cmd = ShellCommand::new("codesign").args(["--force", "--sign", "-"]);
        assert_eq!(format!("{cmd}"), "codesign --force --sign -");
    }

    #[test]
    fn display_renders_env_prefix() {
        let cmd = ShellCommand::new("swift").arg("build").envs(
            &[("PKG_CONFIG_PATH".to_string(), "/opt/lib".to_string())]
                .into_iter()
                .collect(),
        );
        assert_eq!(format!("{cmd}"), "PKG_CONFIG_PATH=/opt/lib swift build");
    }

    #[test]
    fn display_renders_arg_group_inline() {
        // Argument groups render as a single space-joined unit (visually
        // underlined in a real terminal; structurally still all of them).
        let cmd = ShellCommand::new("swift").arg("build").arg_group(&[
            "-Xlinker",
            "-rpath",
            "-Xlinker",
            "@executable_path/../Frameworks",
        ]);
        assert_eq!(
            strip_str(&format!("{cmd}")).to_string(),
            "swift build -Xlinker -rpath -Xlinker @executable_path/../Frameworks"
        );
    }

    #[test]
    fn command_expands_arg_groups_into_separate_argv_entries() {
        // Visual grouping in Display must not bleed into the actual process
        // arguments, argv must still be split per element.
        let cmd = ShellCommand::new("echo")
            .arg("a")
            .arg_group(["b", "c"])
            .arg("d");
        let process_cmd = cmd.command();
        let args: Vec<&std::ffi::OsStr> = process_cmd.get_args().collect();
        let args: Vec<&str> = args.iter().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(args, vec!["a", "b", "c", "d"]);
    }

    #[test]
    fn from_slice_uses_first_element_as_program() {
        let cmd: ShellCommand = (&["plutil", "-convert", "xml1"][..]).into();
        assert_eq!(cmd.program, "plutil");
        assert_eq!(format!("{cmd}"), "plutil -convert xml1");
    }

    #[test]
    fn display_redacts_secret_arg() {
        let secret: SecretString = "supersecretpassword".into();
        let cmd = ShellCommand::new("security")
            .args(["import", "-P"])
            .arg(secret);
        let s = format!("{cmd}");
        assert_eq!(s, "security import -P <redacted>");
    }

    #[test]
    fn command_exposes_secret_value_to_process() {
        let secret: SecretString = "supersecretpassword".into();
        let cmd = ShellCommand::new("security")
            .args(["import", "-P"])
            .arg(secret);
        let process_cmd = cmd.command();
        let args: Vec<&str> = process_cmd
            .get_args()
            .map(|a| a.to_str().unwrap())
            .collect();
        assert_eq!(args, vec!["import", "-P", "supersecretpassword"]);
    }
}
