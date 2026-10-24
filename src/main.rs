mod core;
mod icons;
mod peripherals;
mod search_style;
mod system;
mod ui;

use std::process::ExitCode;

use crate::core::KeyboardLayout;

const HELP: &str = "\
keywrld — keyboard-shaped system shortcut help

Usage:
  keywrld [--layout auto|full|100|tkl|80|75|65|60]
  keywrld --help
  keywrld --version
";

fn parse_args() -> Result<Option<KeyboardLayout>, String> {
    let mut args = std::env::args().skip(1);
    let mut layout = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{HELP}");
                std::process::exit(0);
            }
            "-V" | "--version" => {
                println!("keywrld {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            "--layout" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--layout requires a value".to_string())?;
                layout = KeyboardLayout::parse_override(&value)?;
            }
            value if value.starts_with("--layout=") => {
                layout = KeyboardLayout::parse_override(&value[9..])?;
            }
            _ => return Err(format!("unknown argument: {arg}\n\n{HELP}")),
        }
    }

    Ok(layout)
}

fn main() -> ExitCode {
    let layout_override = match parse_args() {
        Ok(layout) => layout,
        Err(error) => {
            eprintln!("keywrld: {error}");
            return ExitCode::from(2);
        }
    };

    ui::run(layout_override).into()
}
