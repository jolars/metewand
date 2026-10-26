//! Attribution of resolved parameter failures to authored YAML values.

use std::collections::BTreeMap;

use crate::{
    canonical::CanonicalValue,
    manifest::{ExperimentCase, ExperimentDefinition, Manifest, Name, ParameterAxis, SourceSpan},
    parameters::ParameterResolutionError,
    planning::ConfigurationNamespace,
    problem_contract::ProblemContract,
    schema::SchemaValidationError,
    yaml::{SourceMap, escape_pointer},
};

struct Origin<'a> {
    map: &'a SourceMap,
    pointer: String,
}

impl Origin<'_> {
    fn contains(&self, suffix: &str) -> bool {
        self.map.contains(&format!("{}{suffix}", self.pointer))
    }

    fn span(&self, suffix: &str) -> Option<SourceSpan> {
        self.map.span(&format!("{}{suffix}", self.pointer))
    }
}

pub(crate) struct ParameterSources<'a> {
    declaration: Origin<'a>,
    defaults: Origin<'a>,
    experiment: Origin<'a>,
    case: Option<Origin<'a>>,
}

impl<'a> ParameterSources<'a> {
    pub(crate) fn new(
        manifest: &'a Manifest,
        contract: &'a ProblemContract,
        experiment: &ExperimentDefinition,
        case: Option<&ExperimentCase>,
        namespace: ConfigurationNamespace,
        definition: &Name,
    ) -> Self {
        let name = escape_pointer(definition.as_str());
        let (declaration, namespace) = match namespace {
            ConfigurationNamespace::Problem => (
                Origin {
                    map: &contract.source_map,
                    pointer: String::new(),
                },
                "problem-parameters",
            ),
            ConfigurationNamespace::Dataset => (
                Origin {
                    map: &manifest.source_map,
                    pointer: format!("/datasets/{name}"),
                },
                "dataset-parameters",
            ),
            ConfigurationNamespace::Implementation => (
                Origin {
                    map: &manifest.source_map,
                    pointer: format!("/implementations/{name}"),
                },
                "implementation-parameters",
            ),
        };
        let defaults = if namespace == "problem-parameters"
            && manifest.problems[definition].parameter_defaults.is_some()
        {
            Origin {
                map: &manifest.source_map,
                pointer: format!("/problems/{name}"),
            }
        } else {
            Origin {
                map: declaration.map,
                pointer: declaration.pointer.clone(),
            }
        };
        let index = manifest
            .experiments
            .iter()
            .position(|item| item.name == experiment.name)
            .expect("an expanded experiment belongs to the manifest");
        let pointer = format!("/experiments/{index}");
        let suffix = if namespace == "problem-parameters" {
            format!("/{namespace}")
        } else {
            format!("/{namespace}/{name}")
        };
        let case = case.map(|case| {
            let index = experiment
                .cases
                .iter()
                .position(|item| item.name == case.name)
                .expect("an expanded case belongs to the experiment");
            Origin {
                map: &manifest.source_map,
                pointer: format!("{pointer}/cases/{index}{suffix}"),
            }
        });
        Self {
            declaration,
            defaults,
            experiment: Origin {
                map: &manifest.source_map,
                pointer: format!("{pointer}{suffix}"),
            },
            case,
        }
    }

    pub(crate) fn locate(
        &self,
        error: &ParameterResolutionError,
        supplied: &CanonicalValue,
        defaults: Option<&CanonicalValue>,
        axes: &BTreeMap<&str, &ParameterAxis>,
    ) -> Option<SourceSpan> {
        let ParameterResolutionError::Validation(SchemaValidationError::InvalidInstance {
            violations,
            ..
        }) = error
        else {
            return self.defaults.span("/parameter-defaults");
        };
        let pointer = &violations.first()?.value_path;
        let Some(path) = pointer.strip_prefix('/') else {
            return self
                .experiment
                .span("")
                .or_else(|| self.declaration.span(""));
        };
        let encoded = path.split('/').next()?;
        let name = encoded.replace("~1", "/").replace("~0", "~");
        let tail = &path[encoded.len()..];
        if supplied.as_json().pointer(pointer).is_some() {
            let axis = axes.get(name.as_str())?;
            let suffix = format!("/{encoded}");
            let (origin, mut suffix) = if let Some(case) = &self.case
                && case.contains(&suffix)
            {
                (case, suffix)
            } else if self.experiment.contains(&suffix) {
                (&self.experiment, suffix)
            } else {
                (&self.declaration, format!("/parameters{suffix}"))
            };
            match axis {
                ParameterAxis::Value(_) if origin.contains(&format!("{suffix}/value")) => {
                    suffix.push_str("/value")
                }
                ParameterAxis::Value(_) => {}
                ParameterAxis::Grid(values) => {
                    let index = values
                        .iter()
                        .position(|value| value.as_json() == &supplied.as_json()[&name])?;
                    suffix.push_str(&format!("/grid/{index}"));
                }
            }
            return origin.span(&format!("{suffix}{tail}"));
        }
        if defaults.is_some_and(|defaults| defaults.as_json().pointer(pointer).is_some()) {
            let suffix = if self.defaults.contains("/parameters") {
                format!("/parameters/{encoded}/default{tail}")
            } else {
                format!("/parameter-defaults{pointer}")
            };
            return self.defaults.span(&suffix);
        }
        // A missing value belongs to its declaration, even when another axis
        // or a case supplied the rest of the resolved parameter object.
        if self.declaration.contains("/parameters") {
            self.declaration.map.field_span(&format!(
                "{}/parameters/{encoded}",
                self.declaration.pointer
            ))
        } else {
            self.declaration.span("/parameter-schema")
        }
    }
}
