use crate::analyze::analyze_file;
use crate::diagnostic::Diagnostic;
use crate::lower::lower_file;
use crate::parser::parse_file;
use crate::serialize::render_cell;
use crate::survey::{
    check_analyze_dir, check_lex_dir, check_lower_dir, check_parse_dir, survey_dir,
};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

pub fn run() -> Result<(), Diagnostic> {
    let mut args = env::args().skip(1);
    let Some(command) = args.next() else {
        return Err(usage_error("expected a subcommand"));
    };
    match command.as_str() {
        "lex" => {
            let input = args
                .next()
                .ok_or_else(|| usage_error("lex requires <input.sv>"))?;
            if args.next().is_some() {
                return Err(usage_error("lex accepts exactly one input path"));
            }
            let path = PathBuf::from(input);
            let contents = fs::read_to_string(&path).map_err(|err| {
                Diagnostic::new(
                    crate::diagnostic::Span::new(&path, 1, 1),
                    format!("failed to read file: {}", err),
                )
            })?;
            let tokens = crate::lexer::lex_file(&path, &contents)?;
            println!("lex ok: {} tokens from {}", tokens.len(), path.display());
            Ok(())
        }
        "parse" => {
            let input = args
                .next()
                .ok_or_else(|| usage_error("parse requires <input.sv>"))?;
            if args.next().is_some() {
                return Err(usage_error("parse accepts exactly one input path"));
            }
            let path = PathBuf::from(input);
            let contents = fs::read_to_string(&path).map_err(|err| {
                Diagnostic::new(
                    crate::diagnostic::Span::new(&path, 1, 1),
                    format!("failed to read file: {}", err),
                )
            })?;
            let design = parse_file(&path, &contents)?;
            println!("{:#?}", design);
            Ok(())
        }
        "analyze" => {
            let input = args
                .next()
                .ok_or_else(|| usage_error("analyze requires <input.sv>"))?;
            if args.next().is_some() {
                return Err(usage_error("analyze accepts exactly one input path"));
            }
            let path = PathBuf::from(input);
            let contents = fs::read_to_string(&path).map_err(|err| {
                Diagnostic::new(
                    crate::diagnostic::Span::new(&path, 1, 1),
                    format!("failed to read file: {}", err),
                )
            })?;
            let report = analyze_file(&path, &contents)?;
            print!("{}", report.render());
            Ok(())
        }
        "lower" => {
            let input = args
                .next()
                .ok_or_else(|| usage_error("lower requires <input.sv>"))?;
            if args.next().is_some() {
                return Err(usage_error("lower accepts exactly one input path"));
            }
            let path = PathBuf::from(input);
            let contents = fs::read_to_string(&path).map_err(|err| {
                Diagnostic::new(
                    crate::diagnostic::Span::new(&path, 1, 1),
                    format!("failed to read file: {}", err),
                )
            })?;
            let lowered = lower_file(&path, &contents)?;
            println!("{:#?}", lowered);
            Ok(())
        }
        "convert-file" => {
            let input = args
                .next()
                .ok_or_else(|| usage_error("convert-file requires <input.sv> <output.cell>"))?;
            let output = args
                .next()
                .ok_or_else(|| usage_error("convert-file requires <input.sv> <output.cell>"))?;
            let mut dry_run = false;
            for arg in args.by_ref() {
                match arg.as_str() {
                    "--dry-run" => dry_run = true,
                    other => return Err(usage_error(&format!("unexpected argument `{}`", other))),
                }
            }
            let input_path = PathBuf::from(&input);
            let output_path = PathBuf::from(&output);
            let contents = fs::read_to_string(&input_path).map_err(|err| {
                Diagnostic::new(
                    crate::diagnostic::Span::new(&input_path, 1, 1),
                    format!("failed to read file: {}", err),
                )
            })?;
            let lowered = lower_file(&input_path, &contents)?;
            let rendered = render_cell(&lowered.cell);
            if dry_run {
                print!("{}", rendered);
            } else {
                if let Some(parent) = output_path.parent() {
                    fs::create_dir_all(parent).map_err(|err| {
                        Diagnostic::new(
                            crate::diagnostic::Span::new(parent, 1, 1),
                            format!("failed to create output directory: {}", err),
                        )
                    })?;
                }
                fs::write(&output_path, rendered).map_err(|err| {
                    Diagnostic::new(
                        crate::diagnostic::Span::new(&output_path, 1, 1),
                        format!("failed to write file: {}", err),
                    )
                })?;
            }
            Ok(())
        }
        "survey" => {
            let input = args
                .next()
                .ok_or_else(|| usage_error("survey requires <input-dir>"))?;
            if args.next().is_some() {
                return Err(usage_error("survey accepts exactly one input path"));
            }
            let report = survey_dir(Path::new(&input))?;
            print!("{}", report.render());
            Ok(())
        }
        "check" => {
            let input = args
                .next()
                .ok_or_else(|| usage_error("check requires <input-dir>"))?;
            let mut stage = None;
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--stage" => {
                        let value = args
                            .next()
                            .ok_or_else(|| usage_error("--stage requires a value"))?;
                        stage = Some(value);
                    }
                    other => return Err(usage_error(&format!("unexpected argument `{}`", other))),
                }
            }
            match stage.as_deref() {
                Some("lex") | None => {
                    let report = check_lex_dir(Path::new(&input))?;
                    print!("{}", report.render());
                    if report.failed == 0 {
                        Ok(())
                    } else {
                        Err(Diagnostic::new(
                            crate::diagnostic::Span::new(&input, 1, 1),
                            format!("{} files failed lexing", report.failed),
                        ))
                    }
                }
                Some("parse") => {
                    let report = check_parse_dir(Path::new(&input))?;
                    print!("{}", report.render());
                    if report.failed == 0 {
                        Ok(())
                    } else {
                        Err(Diagnostic::new(
                            crate::diagnostic::Span::new(&input, 1, 1),
                            format!("{} files failed parsing", report.failed),
                        ))
                    }
                }
                Some("analyze") => {
                    let report = check_analyze_dir(Path::new(&input))?;
                    print!("{}", report.render());
                    if report.failed == 0 {
                        Ok(())
                    } else {
                        Err(Diagnostic::new(
                            crate::diagnostic::Span::new(&input, 1, 1),
                            format!("{} files failed analyzing", report.failed),
                        ))
                    }
                }
                Some("lower") => {
                    let report = check_lower_dir(Path::new(&input))?;
                    print!("{}", report.render());
                    if report.failed == 0 {
                        Ok(())
                    } else {
                        Err(Diagnostic::new(
                            crate::diagnostic::Span::new(&input, 1, 1),
                            format!("{} files failed lowering", report.failed),
                        ))
                    }
                }
                Some(other) => Err(usage_error(&format!(
                    "unsupported stage `{}`; only `lex`, `parse`, `analyze`, and `lower` are available yet",
                    other
                ))),
            }
        }
        other => Err(usage_error(&format!("unknown subcommand `{}`", other))),
    }
}

fn usage_error(message: &str) -> Diagnostic {
    Diagnostic::new(
        crate::diagnostic::Span::new("<cli>", 1, 1),
        format!(
            "{}; supported commands: lex, parse, analyze, lower, convert-file, survey, check --stage lex|parse|analyze|lower",
            message
        ),
    )
}
