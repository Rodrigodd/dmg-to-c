use crate::diagnostic::Diagnostic;
use crate::survey::{check_lex_dir, survey_dir};
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
                Some(other) => Err(usage_error(&format!(
                    "unsupported stage `{}`; only `lex` is available yet",
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
            "{}; supported commands: lex, survey, check --stage lex",
            message
        ),
    )
}
