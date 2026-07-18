use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use sexpr_fmt::{FormatOptions, format_source};

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("{err}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<ExitCode, Box<dyn std::error::Error>> {
    let cli = Cli::parse(env::args().skip(1))?;
    let source = fs::read_to_string(&cli.path)?;
    let formatted = format_source(
        &source,
        FormatOptions {
            width: cli.width,
            max_inline_items: 4,
        },
    )?;

    match cli.mode {
        Mode::Print => {
            print!("{formatted}");
            Ok(ExitCode::SUCCESS)
        }
        Mode::Write => {
            if formatted != source {
                fs::write(&cli.path, formatted)?;
            }
            Ok(ExitCode::SUCCESS)
        }
        Mode::Check => {
            if formatted == source {
                Ok(ExitCode::SUCCESS)
            } else {
                Ok(ExitCode::from(1))
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Print,
    Write,
    Check,
}

#[derive(Debug, Clone)]
struct Cli {
    path: PathBuf,
    mode: Mode,
    width: usize,
}

#[derive(Debug)]
struct CliError(String);

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CliError {}

impl Cli {
    fn parse<I>(args: I) -> Result<Self, CliError>
    where
        I: IntoIterator,
        I::Item: Into<String>,
    {
        let mut mode = Mode::Print;
        let mut width = 100usize;
        let mut path: Option<PathBuf> = None;

        let mut iter = args.into_iter().map(Into::into).peekable();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--write" => mode = Mode::Write,
                "--check" => mode = Mode::Check,
                "--width" => {
                    let value = iter
                        .next()
                        .ok_or_else(|| CliError(String::from("missing value for --width")))?;
                    width = value
                        .parse::<usize>()
                        .map_err(|_| CliError(String::from("invalid width")))?;
                }
                "--help" | "-h" => {
                    return Err(CliError(String::from(
                        "usage: sexpr-fmt [--write|--check] [--width N] FILE",
                    )));
                }
                _ if arg.starts_with('-') => {
                    return Err(CliError(format!("unknown flag: {arg}")));
                }
                _ => {
                    if path.is_some() {
                        return Err(CliError(String::from("expected exactly one file path")));
                    }
                    path = Some(PathBuf::from(arg));
                }
            }
        }

        let path = path.ok_or_else(|| CliError(String::from("missing file path")))?;
        Ok(Self { path, mode, width })
    }
}
