use std::{ffi::OsString, path::PathBuf};

use metewand_runtime::authoring::{
    init_benchmark_with_options,
    scaffold::{ScaffoldComponent, ScaffoldRequest, WorkerLanguage, scaffold_component},
};

use crate::output::{CommandError, CommandResult};

const INIT_HELP: &str = "Usage: metewand [--output FORMAT] init [PATH] [--language python|r] [--empty] [--dry-run]\n\nCreate a runnable two-file benchmark (Python by default). Use --language r for R, or --empty for a neutral manifest to extend with scaffold. Existing files are never replaced. Dependencies are not installed. Use --dry-run to inspect proposed paths.\n";

const SCAFFOLD_HELP: &str = "\
Usage: metewand [--output FORMAT] scaffold <dataset|problem|implementation> NAME
         [--language python|r|rust] [--environment NAME] [--problem NAME]
         [--manifest PATH] [--dry-run]

Add one named component and report every created or modified path. The default
language is python. An omitted environment uses the manifest's local default.
Implementation scaffolds require an existing --problem. Default manifest:
./metewand.yaml.

Generate small worker adapters and compact inline declarations. Implement the
callbacks and declare their data shapes before running. Add scientific rules
and reference cases as needed. Rust adapters must be built as native workers. Existing files and
definitions are never replaced. No package managers or workers are invoked.
Use --dry-run to validate and report proposed changes without writing.
";

pub fn scaffold_command(arguments: &[OsString]) -> Result<CommandResult, CommandError> {
    if arguments == ["-h"]
        || arguments == ["--help"]
        || (arguments.len() == 2 && (arguments[1] == "-h" || arguments[1] == "--help"))
    {
        return Ok(CommandResult::help(SCAFFOLD_HELP));
    }
    let component = arguments
        .first()
        .and_then(|argument| argument.to_str())
        .and_then(ScaffoldComponent::parse)
        .ok_or_else(|| {
            CommandError::usage(
                "invalid_scaffold_component",
                "`scaffold` requires dataset, problem, or implementation",
            )
        })?;
    let text = |argument: &OsString| {
        argument
            .to_str()
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| {
                CommandError::usage(
                    "invalid_scaffold_argument",
                    "scaffold names and paths must be nonempty UTF-8",
                )
            })
    };
    let name = arguments.get(1).ok_or_else(|| {
        CommandError::usage(
            "missing_scaffold_argument",
            "`scaffold` requires a component name",
        )
    })?;
    let name = text(name)?;
    let mut manifest = None;
    let mut environment = None;
    let mut problem = None;
    let mut language = None;
    let mut dry_run = false;
    let mut options = arguments[2..].iter();
    while let Some(option) = options.next() {
        if option == "--dry-run" {
            if dry_run {
                return Err(CommandError::usage(
                    "duplicate_scaffold_flag",
                    "`--dry-run` may be supplied at most once",
                ));
            }
            dry_run = true;
            continue;
        }
        let destination = match option.to_str() {
            Some("--manifest") => &mut manifest,
            Some("--environment") => &mut environment,
            Some("--problem") => &mut problem,
            Some("--language") => &mut language,
            _ => {
                return Err(CommandError::usage(
                    "unexpected_argument",
                    format!("unexpected scaffold option `{}`", option.to_string_lossy()),
                ));
            }
        };
        if destination.is_some() {
            return Err(CommandError::usage(
                "duplicate_scaffold_flag",
                format!(
                    "`{}` may be supplied at most once",
                    option.to_string_lossy()
                ),
            ));
        }
        let value = options.next().ok_or_else(|| {
            CommandError::usage(
                "missing_scaffold_argument",
                format!("`{}` requires a value", option.to_string_lossy()),
            )
        })?;
        *destination = Some(text(value)?);
    }
    let language = language.map_or(Ok(WorkerLanguage::default()), |language| {
        WorkerLanguage::parse(&language).ok_or_else(|| {
            CommandError::usage(
                "invalid_scaffold_language",
                "`--language` requires python, r, or rust",
            )
        })
    })?;
    let request = ScaffoldRequest {
        component,
        name,
        environment,
        language,
        problem,
    };
    scaffold_component(
        &PathBuf::from(manifest.unwrap_or_else(|| "metewand.yaml".to_owned())),
        &request,
        dry_run,
    )
    .map(CommandResult::Scaffold)
    .map_err(|error| CommandError::scaffold(&error))
}

pub fn init_command(arguments: &[OsString]) -> Result<CommandResult, CommandError> {
    if arguments == ["-h"] || arguments == ["--help"] {
        return Ok(CommandResult::help(INIT_HELP));
    }
    let mut directory = None;
    let mut language = None;
    let mut empty = false;
    let mut dry_run = false;
    let mut positional_only = false;
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        if !positional_only {
            if argument == "--" {
                positional_only = true;
                continue;
            }
            if argument == "--language" {
                let value = arguments
                    .next()
                    .and_then(|value| value.to_str())
                    .ok_or_else(|| {
                        CommandError::usage(
                            "missing_init_language",
                            "--language requires python or r",
                        )
                    })?;
                let selected = WorkerLanguage::parse(value)
                    .filter(|value| !matches!(value, WorkerLanguage::Rust))
                    .ok_or_else(|| {
                        CommandError::usage(
                            "invalid_init_language",
                            "--language requires python or r",
                        )
                    })?;
                if language.replace(selected).is_some() {
                    return Err(CommandError::usage(
                        "duplicate_init_flag",
                        "--language may be supplied at most once",
                    ));
                }
                continue;
            }
            if argument == "--empty" {
                if empty {
                    return Err(CommandError::usage(
                        "duplicate_init_flag",
                        "--empty may be supplied at most once",
                    ));
                }
                empty = true;
                continue;
            }
            if argument == "--dry-run" {
                if dry_run {
                    return Err(CommandError::usage(
                        "duplicate_init_flag",
                        "`--dry-run` may be supplied at most once",
                    ));
                }
                dry_run = true;
                continue;
            }
            if argument.to_string_lossy().starts_with('-') {
                return Err(CommandError::usage(
                    "unexpected_argument",
                    format!("unexpected init option `{}`", argument.to_string_lossy()),
                ));
            }
        }
        if argument.is_empty() || argument.to_str().is_none() {
            return Err(CommandError::usage(
                "invalid_init_path",
                "`init` requires a nonempty UTF-8 directory path",
            ));
        }
        if directory.replace(PathBuf::from(argument)).is_some() {
            return Err(CommandError::usage(
                "unexpected_argument",
                "`init` accepts at most one destination directory",
            ));
        }
    }
    if empty && language.is_some() {
        return Err(CommandError::usage(
            "invalid_init_flags",
            "--empty does not select a worker language",
        ));
    }
    init_benchmark_with_options(
        &directory.unwrap_or_else(|| PathBuf::from(".")),
        language.unwrap_or_default(),
        empty,
        dry_run,
    )
    .map(CommandResult::Init)
    .map_err(|error| CommandError::init(&error))
}
