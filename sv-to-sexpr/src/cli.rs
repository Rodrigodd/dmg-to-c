use crate::analyze::analyze_file;
use crate::diagnostic::{Diagnostic, DiagnosticPolicy};
use crate::lower::lower_file;
use crate::parser::parse_file;
use crate::serialize::render_cell;
use crate::survey::{
    CheckReport, check_analyze_dir, check_lex_dir, check_lower_dir, check_parse_dir, survey_dir,
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
            let (input, _policy) = parse_single_input(args, "lex", "<input.sv>")?;
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
            let (input, _policy) = parse_single_input(args, "parse", "<input.sv>")?;
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
            let (input, _policy) = parse_single_input(args, "analyze", "<input.sv>")?;
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
            let (input, _policy) = parse_single_input(args, "lower", "<input.sv>")?;
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
            let parsed = parse_convert_file_args(args)?;
            let input_path = PathBuf::from(&parsed.input);
            let output_path = PathBuf::from(&parsed.output);
            let contents = fs::read_to_string(&input_path).map_err(|err| {
                Diagnostic::new(
                    crate::diagnostic::Span::new(&input_path, 1, 1),
                    format!("failed to read file: {}", err),
                )
            })?;
            let lowered = lower_file(&input_path, &contents)?;
            let rendered = render_cell(&lowered.cell);
            // `lower_file` currently returns errors directly and has no warning
            // channel. Parsing retains the shared policy without inventing one.
            let _policy = parsed.policy;
            if parsed.dry_run {
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
            let (input, _policy) = parse_single_input(args, "survey", "<input-dir>")?;
            let report = survey_dir(Path::new(&input))?;
            print!("{}", report.render());
            Ok(())
        }
        "check" => {
            let parsed = parse_check_args(args)?;
            match parsed.stage {
                CheckStage::Lex => run_check(&parsed.input, "lexing", parsed.policy, check_lex_dir),
                CheckStage::Parse => {
                    run_check(&parsed.input, "parsing", parsed.policy, check_parse_dir)
                }
                CheckStage::Analyze => {
                    run_check(&parsed.input, "analyzing", parsed.policy, check_analyze_dir)
                }
                CheckStage::Lower => {
                    run_check(&parsed.input, "lowering", parsed.policy, check_lower_dir)
                }
            }
        }
        other => Err(usage_error(&format!("unknown subcommand `{}`", other))),
    }
}

fn parse_single_input(
    args: impl Iterator<Item = String>,
    command: &str,
    operand: &str,
) -> Result<(String, DiagnosticPolicy), Diagnostic> {
    let mut input = None;
    let mut strict = false;
    for arg in args {
        match arg.as_str() {
            "--strict" if !strict => strict = true,
            "--strict" => return Err(usage_error("--strict may be specified only once")),
            _ if arg.starts_with('-') => {
                return Err(usage_error(&format!("unknown option `{arg}`")));
            }
            _ if input.is_none() => input = Some(arg),
            _ => {
                return Err(usage_error(&format!(
                    "{command} accepts exactly one input path"
                )));
            }
        }
    }
    let input = input.ok_or_else(|| usage_error(&format!("{command} requires {operand}")))?;
    Ok((input, DiagnosticPolicy::new(strict)))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ConvertFileArgs {
    input: String,
    output: String,
    dry_run: bool,
    policy: DiagnosticPolicy,
}

fn parse_convert_file_args(
    args: impl Iterator<Item = String>,
) -> Result<ConvertFileArgs, Diagnostic> {
    let mut positionals = Vec::new();
    let mut dry_run = false;
    let mut strict = false;
    for arg in args {
        match arg.as_str() {
            "--dry-run" if !dry_run => dry_run = true,
            "--dry-run" => return Err(usage_error("--dry-run may be specified only once")),
            "--strict" if !strict => strict = true,
            "--strict" => return Err(usage_error("--strict may be specified only once")),
            _ if arg.starts_with('-') => {
                return Err(usage_error(&format!("unknown option `{arg}`")));
            }
            _ => positionals.push(arg),
        }
    }
    if positionals.len() < 2 {
        return Err(usage_error(
            "convert-file requires <input.sv> <output.cell>",
        ));
    }
    if positionals.len() > 2 {
        return Err(usage_error(
            "convert-file accepts exactly two path operands",
        ));
    }
    Ok(ConvertFileArgs {
        input: positionals.remove(0),
        output: positionals.remove(0),
        dry_run,
        policy: DiagnosticPolicy::new(strict),
    })
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum CheckStage {
    #[default]
    Lex,
    Parse,
    Analyze,
    Lower,
}

impl CheckStage {
    fn parse(value: &str) -> Result<Self, Diagnostic> {
        match value {
            "lex" => Ok(Self::Lex),
            "parse" => Ok(Self::Parse),
            "analyze" => Ok(Self::Analyze),
            "lower" => Ok(Self::Lower),
            other => Err(usage_error(&format!(
                "unsupported stage `{other}`; expected lex, parse, analyze, or lower"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CheckArgs {
    input: String,
    stage: CheckStage,
    policy: DiagnosticPolicy,
}

fn parse_check_args(args: impl Iterator<Item = String>) -> Result<CheckArgs, Diagnostic> {
    let mut args = args.peekable();
    let mut input = None;
    let mut stage = None;
    let mut strict = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--strict" if !strict => strict = true,
            "--strict" => return Err(usage_error("--strict may be specified only once")),
            "--stage" if stage.is_some() => {
                return Err(usage_error("--stage may be specified only once"));
            }
            "--stage" => {
                let value = args
                    .next()
                    .filter(|value| !value.starts_with('-'))
                    .ok_or_else(|| usage_error("--stage requires a value"))?;
                stage = Some(CheckStage::parse(&value)?);
            }
            _ if arg.starts_with('-') => {
                return Err(usage_error(&format!("unknown option `{arg}`")));
            }
            _ if input.is_none() => input = Some(arg),
            _ => return Err(usage_error("check accepts exactly one input path")),
        }
    }
    let input = input.ok_or_else(|| usage_error("check requires <input-dir>"))?;
    Ok(CheckArgs {
        input,
        stage: stage.unwrap_or_default(),
        policy: DiagnosticPolicy::new(strict),
    })
}

fn run_check(
    input: &str,
    action: &str,
    policy: DiagnosticPolicy,
    check: fn(&Path) -> Result<CheckReport, Diagnostic>,
) -> Result<(), Diagnostic> {
    let report = check(Path::new(input))?;
    print!("{}", report.render());
    if report.fails(policy) {
        Err(Diagnostic::new(
            crate::diagnostic::Span::new(input, 1, 1),
            check_failure_message(&report, action, policy),
        ))
    } else {
        Ok(())
    }
}

fn check_failure_message(
    report: &crate::survey::CheckReport,
    action: &str,
    policy: DiagnosticPolicy,
) -> String {
    if report.failed() > 0 {
        format!("{} files failed {}", report.failed(), action)
    } else if policy.strict && report.warned() > 0 {
        format!(
            "{} files warned during {} in strict mode",
            report.warned(),
            action
        )
    } else {
        "check failed diagnostic policy".to_string()
    }
}

fn usage_error(message: &str) -> Diagnostic {
    Diagnostic::new(
        crate::diagnostic::Span::new("<cli>", 1, 1),
        format!(
            "{}; supported commands: lex, parse, analyze, lower, convert-file, survey, check --stage lex|parse|analyze|lower; diagnostic-capable commands accept --strict",
            message
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args<'a>(values: &'a [&'a str]) -> impl Iterator<Item = String> + 'a {
        values.iter().map(|value| (*value).to_string())
    }

    fn message(error: Result<impl std::fmt::Debug, Diagnostic>) -> String {
        error.unwrap_err().message
    }

    #[test]
    fn single_input_flags_are_order_independent() {
        let (path, policy) =
            parse_single_input(args(&["--strict", "gate.sv"]), "lower", "<input.sv>").unwrap();
        assert_eq!(path, "gate.sv");
        assert!(policy.strict);

        let (path, policy) =
            parse_single_input(args(&["gate.sv", "--strict"]), "lower", "<input.sv>").unwrap();
        assert_eq!(path, "gate.sv");
        assert!(policy.strict);
    }

    #[test]
    fn single_input_rejects_duplicate_unknown_missing_and_extra_arguments() {
        assert!(
            message(parse_single_input(
                args(&["gate.sv", "--strict", "--strict"]),
                "lower",
                "<input.sv>",
            ))
            .starts_with("--strict may be specified only once")
        );
        assert!(
            message(parse_single_input(
                args(&["--mystery", "gate.sv"]),
                "lower",
                "<input.sv>",
            ))
            .starts_with("unknown option `--mystery`")
        );
        assert!(
            message(parse_single_input(args(&[]), "lower", "<input.sv>",))
                .starts_with("lower requires <input.sv>")
        );
        assert!(
            message(parse_single_input(
                args(&["one.sv", "two.sv"]),
                "lower",
                "<input.sv>",
            ))
            .starts_with("lower accepts exactly one input path")
        );
    }

    #[test]
    fn convert_file_parses_flags_in_any_position_without_claiming_warning_output() {
        let parsed =
            parse_convert_file_args(args(&["--dry-run", "input.sv", "--strict", "output.cell"]))
                .unwrap();
        assert_eq!(parsed.input, "input.sv");
        assert_eq!(parsed.output, "output.cell");
        assert!(parsed.dry_run);
        assert!(parsed.policy.strict);
    }

    #[test]
    fn convert_file_rejects_duplicate_unknown_missing_and_extra_arguments() {
        assert!(
            message(parse_convert_file_args(args(&[
                "input.sv",
                "output.cell",
                "--strict",
                "--strict",
            ])))
            .starts_with("--strict may be specified only once")
        );
        assert!(
            message(parse_convert_file_args(args(&[
                "input.sv",
                "output.cell",
                "--dry-run",
                "--dry-run",
            ])))
            .starts_with("--dry-run may be specified only once")
        );
        assert!(
            message(parse_convert_file_args(args(&[
                "--mystery",
                "input.sv",
                "output.cell",
            ])))
            .starts_with("unknown option `--mystery`")
        );
        assert!(
            message(parse_convert_file_args(args(&["input.sv"])))
                .starts_with("convert-file requires <input.sv> <output.cell>")
        );
        assert!(
            message(parse_convert_file_args(args(&[
                "input.sv",
                "output.cell",
                "extra.cell",
            ])))
            .starts_with("convert-file accepts exactly two path operands")
        );
    }

    #[test]
    fn check_flags_are_order_independent_and_stage_defaults_to_lex() {
        let parsed = parse_check_args(args(&["--stage", "lower", "--strict", "sv-cells"])).unwrap();
        assert_eq!(parsed.input, "sv-cells");
        assert_eq!(parsed.stage, CheckStage::Lower);
        assert!(parsed.policy.strict);

        let parsed = parse_check_args(args(&["sv-cells", "--strict", "--stage", "parse"])).unwrap();
        assert_eq!(parsed.stage, CheckStage::Parse);

        assert_eq!(
            parse_check_args(args(&["sv-cells"])).unwrap().stage,
            CheckStage::Lex
        );
    }

    #[test]
    fn check_rejects_duplicate_unknown_missing_extra_and_unsupported_arguments() {
        assert!(
            message(parse_check_args(args(&[
                "sv-cells", "--strict", "--strict",
            ])))
            .starts_with("--strict may be specified only once")
        );
        assert!(
            message(parse_check_args(args(&[
                "sv-cells", "--stage", "lex", "--stage", "parse",
            ])))
            .starts_with("--stage may be specified only once")
        );
        assert!(
            message(parse_check_args(args(&["--mystery", "sv-cells"])))
                .starts_with("unknown option `--mystery`")
        );
        assert!(message(parse_check_args(args(&[]))).starts_with("check requires <input-dir>"));
        assert!(
            message(parse_check_args(args(&["one", "two"])))
                .starts_with("check accepts exactly one input path")
        );
        assert!(
            message(parse_check_args(args(&["sv-cells", "--stage"])))
                .starts_with("--stage requires a value")
        );
        assert!(
            message(parse_check_args(args(
                &["sv-cells", "--stage", "--strict",]
            )))
            .starts_with("--stage requires a value")
        );
        assert!(
            message(parse_check_args(args(&[
                "sv-cells",
                "--stage",
                "serialize",
            ])))
            .starts_with("unsupported stage `serialize`")
        );
    }

    #[test]
    fn check_failure_messages_follow_report_policy_counts() {
        let mut warning = CheckReport::new("lower");
        warning.record(Diagnostic::warning(
            crate::diagnostic::Span::new("warning.sv", 1, 1),
            "approximation",
        ));
        assert_eq!(
            check_failure_message(&warning, "lowering", DiagnosticPolicy::new(true)),
            "1 files warned during lowering in strict mode"
        );

        warning.record(Diagnostic::error(
            crate::diagnostic::Span::new("error.sv", 1, 1),
            "unsupported",
        ));
        assert_eq!(
            check_failure_message(&warning, "lowering", DiagnosticPolicy::new(true)),
            "1 files failed lowering"
        );
    }
}
