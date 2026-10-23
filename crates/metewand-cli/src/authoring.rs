use std::{ffi::OsString, path::PathBuf};

use metewand_runtime::authoring::{
    init_benchmark,
    scaffold::{ScaffoldComponent, ScaffoldRequest, scaffold_component},
};

use crate::output::{CommandError, CommandResult};

const INIT_HELP: &str = "Usage: metewand [--output FORMAT] init [PATH] [--dry-run]\n\nCreate only PATH/metewand.yaml (default PATH: .), containing a version and a neutral benchmark name. Report every created path, including missing directories. Refuse an existing manifest, including symlinks. Use --dry-run to validate and report proposed paths without writing. Use -- before a path beginning with a hyphen.\n";

const SCAFFOLD_HELP: &str = "\
Usage: metewand [--output FORMAT] scaffold <dataset|problem|implementation> NAME
         --environment NAME [--problem NAME] [--manifest PATH] [--dry-run]

Add one named component and report every created or modified path. Select an
existing environment explicitly. Implementation scaffolds also require an
existing --problem. Default manifest: ./metewand.yaml.

Generate raw-command worker stubs and rejecting schema placeholders. Problem
contracts require author-supplied scientific semantics and reference cases.
Edit these files before checking or running the component. Existing files and
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
    let environment = environment.ok_or_else(|| {
        CommandError::usage(
            "missing_scaffold_argument",
            "select an existing worker environment with `--environment NAME`",
        )
    })?;
    let request = ScaffoldRequest {
        component,
        name,
        environment,
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
    let mut dry_run = false;
    let mut positional_only = false;
    for argument in arguments {
        if !positional_only {
            if argument == "--" {
                positional_only = true;
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
    init_benchmark(&directory.unwrap_or_else(|| PathBuf::from(".")), dry_run)
        .map(CommandResult::Init)
        .map_err(|error| CommandError::init(&error))
}
