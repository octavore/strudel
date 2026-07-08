use std::error::Error;
use std::{env, process};

use icon::{IconOptions, generate, parse_hex_color};

fn main() {
    let mut args = env::args().skip(1);
    let (Some(foreground_path), Some(background_hex), Some(output_path)) =
        (args.next(), args.next(), args.next())
    else {
        eprintln!("usage: generate <foreground.png> <#RRGGBB> <output.png> [foreground_scale]");
        process::exit(1);
    };
    let foreground_scale = args.next();

    if let Err(err) = run(
        &foreground_path,
        &background_hex,
        &output_path,
        foreground_scale.as_deref(),
    ) {
        eprintln!("error: {err}");
        process::exit(1);
    }
}

fn run(
    foreground_path: &str,
    background_hex: &str,
    output_path: &str,
    foreground_scale: Option<&str>,
) -> Result<(), Box<dyn Error>> {
    let foreground = image::open(foreground_path)?.into_rgba8();
    let background = parse_hex_color(background_hex)?;
    let mut options = IconOptions {
        background,
        ..IconOptions::default()
    };
    if let Some(scale) = foreground_scale {
        options.foreground_scale = scale.parse()?;
    }
    let icon = generate(&foreground, &options)?;
    icon.save(output_path)?;
    println!("wrote {output_path}");
    Ok(())
}
