mod apple;
mod builder;
mod cli;
mod config;
mod devices;
mod help;
mod icon;
mod init;
mod paths;
mod shell;
mod skill;
mod status;

use clml::ceprintln;

fn main() -> ! {
    let exit_code = cli::Cli::execute().map(|_| 0).unwrap_or_else(|e| {
        ceprintln!("<red>Error: {e:#}</red>");
        1
    });

    std::process::exit(exit_code);
}
