//! Shared iocraft layout for human comparison output.

use crate::core::comparison::ComparisonRow;
use iocraft::prelude::*;
use std::io::{self, IsTerminal};

pub(super) fn width() -> usize {
    if io::stdout().is_terminal() {
        crossterm::terminal::size()
            .ok()
            .filter(|(w, _)| *w > 0)
            .map(|(w, _)| usize::from(w).saturating_sub(2).clamp(1, 110))
            .unwrap_or(110)
    } else {
        110
    }
}

fn render(mut element: AnyElement<'_>, width: usize) -> String {
    let canvas = element.render(Some(width));
    if io::stdout().is_terminal()
        && std::env::var_os("NO_COLOR").is_none()
        && std::env::var("TERM").as_deref() != Ok("dumb")
    {
        let mut output = Vec::new();
        canvas
            .write_ansi(&mut output)
            .expect("writing a canvas to memory");
        String::from_utf8(output).expect("iocraft renders UTF-8")
    } else {
        canvas.to_string()
    }
}

#[derive(Clone)]
struct Cell {
    text: String,
    weight: f32,
    numeric: bool,
    color: Option<Color>,
}
fn cell(text: impl Into<String>, weight: f32, numeric: bool) -> Cell {
    Cell {
        text: text.into(),
        weight,
        numeric,
        color: None,
    }
}

fn table_row(cells: Vec<Cell>, header: bool) -> AnyElement<'static> {
    let mut elements = Vec::new();
    for (index, cell) in cells.into_iter().enumerate() {
        if index > 0 {
            elements.push(
                element! {
                    View(width: 1, flex_shrink: 0.0_f32) { Text(content: "|") }
                }
                .into_any(),
            );
        }
        elements.push(
            element! {
                View(
                    flex_direction: FlexDirection::Column,
                    flex_basis: FlexBasis::Length(0),
                    flex_grow: cell.weight,
                    min_width: 0,
                    padding_left: 1,
                    padding_right: 1,
                ) {
                    Text(
                        content: cell.text,
                        align: if cell.numeric { TextAlign::Right } else { TextAlign::Left },
                        weight: if header { Weight::Bold } else { Weight::Normal },
                        color: if header { Some(Color::Cyan) } else { cell.color },
                    )
                }
            }
            .into_any(),
        );
    }
    element! {
        View(width: 100pct, flex_direction: FlexDirection::Row, align_items: AlignItems::Stretch) {
            #(elements)
        }
    }
    .into()
}

fn context(text: String) -> AnyElement<'static> {
    element!(Text(content:text)).into()
}

fn same_context(a: &ComparisonRow, b: &ComparisonRow) -> bool {
    a.experiment == b.experiment
        && a.dataset == b.dataset
        && a.problem == b.problem
        && a.case == b.case
        && a.timing_scope == b.timing_scope
        && a.primary_time == b.primary_time
        && a.parameters["dataset"] == b.parameters["dataset"]
        && a.parameters["problem"] == b.parameters["problem"]
}

fn duration(value: Option<u64>, status: &str) -> String {
    let value = value
        .map(|n| format!("{:.3}", n as f64 / 1_000_000.0))
        .unwrap_or_else(|| "unavailable".into());
    if status == "partial" {
        format!("{value} (partial)")
    } else {
        value
    }
}

pub(super) fn comparison(rows: &[ComparisonRow]) -> String {
    comparison_at_width(rows, width())
}

fn comparison_at_width(rows: &[ComparisonRow], width: usize) -> String {
    if rows.is_empty() {
        return String::new();
    }
    // Stored candidate identities do not put matching comparison contexts together.
    let mut ordered = rows.iter().collect::<Vec<_>>();
    ordered.sort_by_cached_key(|row| {
        (
            (
                row.experiment.as_deref(),
                row.dataset.as_deref(),
                row.problem.as_deref(),
                row.case.as_deref(),
                row.timing_scope.as_deref(),
                row.primary_time.as_deref(),
                row.parameters["dataset"].to_string(),
                row.parameters["problem"].to_string(),
            ),
            (
                row.implementation.as_deref().unwrap_or(&row.candidate_id),
                row.parameters["implementation"].to_string(),
                row.implementation_repetition,
                row.measurement_index,
                row.retry_index,
            ),
        )
    });
    let mut rows = ordered.as_slice();
    let mut output = String::from("\nComparison (individual measurements)\n");
    while let Some(first) = rows.first() {
        let count = rows
            .iter()
            .take_while(|row| same_context(first, row))
            .count();
        let group = &rows[..count];
        rows = &rows[count..];
        let retry_shared = group.iter().all(|row| row.retry_index == first.retry_index);
        let completion_shared = group.iter().all(|row| row.completion == first.completion);
        let parameters_shared = group
            .iter()
            .all(|row| row.parameters["implementation"] == first.parameters["implementation"]);
        let metrics_shared = group.iter().all(|row| row.metrics == first.metrics);
        let mut context_lines = vec![
            "Rep = implementation repetition:measurement".into(),
            format!(
                "experiment: {}; dataset: {}; problem: {}; case: {}",
                first.experiment.as_deref().unwrap_or("unavailable"),
                first.dataset.as_deref().unwrap_or("unavailable"),
                first.problem.as_deref().unwrap_or("unavailable"),
                first
                    .case
                    .as_deref()
                    .unwrap_or(if first.experiment.is_some() {
                        "default"
                    } else {
                        "unavailable"
                    }),
            ),
            format!(
                "timing: {}; primary: {}",
                first.timing_scope.as_deref().unwrap_or("unavailable"),
                first.primary_time.as_deref().unwrap_or("unavailable")
            ),
            format!(
                "dataset parameters: {}; problem parameters: {}",
                first.parameters["dataset"], first.parameters["problem"]
            ),
        ];
        if retry_shared {
            context_lines.push(format!("retry: {}", first.retry_index));
        }
        if completion_shared {
            context_lines.push(format!(
                "completion: {}",
                first.completion.as_deref().unwrap_or("unavailable")
            ));
        }
        if parameters_shared {
            context_lines.push(format!(
                "implementation parameters: {}",
                first.parameters["implementation"]
            ));
        }
        if metrics_shared {
            context_lines.push(format!("metrics (all rows): {}", first.metrics));
        }
        let header = vec![
            cell("Method", 3.0, false),
            cell("Rep", 1.0, true),
            cell("Status", 2.0, false),
            cell("Primary (ms)", 2.0, true),
            cell("Wall (ms)", 2.0, true),
        ];
        let extras = |mut cells: Vec<Cell>, row: Option<&ComparisonRow>| {
            if !retry_shared {
                cells.push(cell(
                    row.map_or_else(|| "Retry".into(), |r| r.retry_index.to_string()),
                    1.0,
                    true,
                ));
            }
            if !completion_shared {
                cells.push(cell(
                    row.map_or_else(
                        || "Completion".into(),
                        |r| r.completion.clone().unwrap_or_else(|| "unavailable".into()),
                    ),
                    2.0,
                    false,
                ));
            }
            cells
        };
        let mut elements = vec![
            context(context_lines.join("\n")),
            table_row(extras(header, None), true),
        ];
        for row in group {
            let mut status = cell(&row.status, 2.0, false);
            status.color = Some(match row.status.as_str() {
                "accepted" => Color::Green,
                "failed" => Color::Red,
                _ => Color::Yellow,
            });
            let repetition = match (row.implementation_repetition, row.measurement_index) {
                (Some(i), Some(m)) => format!("{}:{}", i + 1, m + 1),
                _ => "?".into(),
            };
            elements.push(table_row(
                extras(
                    vec![
                        cell(
                            row.implementation.as_deref().unwrap_or(&row.candidate_id),
                            3.0,
                            false,
                        ),
                        cell(repetition, 1.0, true),
                        status,
                        cell(
                            duration(row.primary_time_ns, &row.primary_time_status),
                            2.0,
                            true,
                        ),
                        cell(
                            duration(row.timed_wall_time_ns, &row.timed_wall_time_status),
                            2.0,
                            true,
                        ),
                    ],
                    Some(row),
                ),
                false,
            ));
            if !parameters_shared {
                elements.push(element!(View(padding_left:2) {Text(content:format!("parameters: {}",row.parameters["implementation"]))}).into());
            }
            if !metrics_shared {
                elements.push(element!(View(padding_left:2) {Text(content:format!("metrics: {}",row.metrics))}).into());
            }
            if let Some(time) = row.instrumented_time_ns {
                elements.push(context(format!(
                    "instrumented: {time} ns; scope: {}",
                    row.instrumented_scope.as_deref().unwrap_or("unavailable")
                )));
            }
            if let Some(diagnostic) = &row.diagnostic {
                elements.push(context(format!(
                    "failure [{}]: {}",
                    diagnostic.code, diagnostic.message
                )));
            }
        }
        output.push_str(&render(
            element!(View(width:100pct,flex_direction:FlexDirection::Column) {#(elements)}).into(),
            width,
        ));
        output.push('\n');
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(method: &str, bias: u64) -> ComparisonRow {
        ComparisonRow {
            profile: None,
            candidate_id: method.into(),
            attempt_id: method.into(),
            observation_id: None,
            experiment: Some("main".into()),
            case: None,
            dataset: Some("numbers".into()),
            problem: Some("sum".into()),
            implementation: Some(method.into()),
            parameters: serde_json::json!({"dataset":{},"problem":{},"implementation":{"bias":bias}}),
            implementation_repetition: Some(0),
            measurement_index: Some(0),
            retry_index: 0,
            status: "accepted".into(),
            valid: Some(true),
            completion: Some("reached".into()),
            selected: Some(true),
            metrics: serde_json::json!({"absolute_error":bias}),
            primary_time: Some("timed_wall_time".into()),
            primary_time_ns: Some(2_041_000),
            primary_time_status: "complete".into(),
            timed_wall_time_ns: Some(2_041_000),
            timed_wall_time_status: "complete".into(),
            timing_scope: Some("prepare_and_execute".into()),
            instrumented_time_ns: None,
            instrumented_scope: None,
            diagnostic: None,
        }
    }

    #[test]
    fn interleaved_comparisons_group_datasets_and_sort_measurements() {
        let mut rows = Vec::new();
        for dataset in ["small", "large", "medium"] {
            for method in ["reduce_sum", "builtin_sum"] {
                for measurement in [1, 0] {
                    let mut row = row(method, 0);
                    row.dataset = Some(dataset.into());
                    row.measurement_index = Some(measurement);
                    rows.push(row);
                }
            }
        }
        let output = comparison_at_width(&rows, 110);
        rows.reverse();
        assert_eq!(output, comparison_at_width(&rows, 110));
        let groups = output
            .split("experiment: main; dataset: ")
            .skip(1)
            .collect::<Vec<_>>();
        assert_eq!(groups.len(), 3, "{output}");
        for (group, dataset) in groups.iter().zip(["large", "medium", "small"]) {
            assert!(group.starts_with(dataset), "{group}");
            let measurements = group
                .lines()
                .filter(|line| line.contains('|') && !line.contains("Method"))
                .map(|line| line.split('|').map(str::trim).collect::<Vec<_>>())
                .collect::<Vec<_>>();
            let labels = measurements
                .iter()
                .map(|cells| (cells[0], cells[1]))
                .collect::<Vec<_>>();
            assert_eq!(
                labels,
                [
                    ("builtin_sum", "1:1"),
                    ("builtin_sum", "1:2"),
                    ("reduce_sum", "1:1"),
                    ("reduce_sum", "1:2")
                ],
                "{group}"
            );
        }
    }

    #[test]
    fn dataset_parameter_configurations_remain_separate() {
        let mut small = row("builtin_sum", 0);
        small.parameters["dataset"] = serde_json::json!({"size": 100});
        let mut large = small.clone();
        large.parameters["dataset"] = serde_json::json!({"size": 1000});
        let mut repeated = small.clone();
        repeated.measurement_index = Some(1);
        let output = comparison_at_width(&[small, large, repeated], 110);
        assert_eq!(output.matches("experiment: main;").count(), 2, "{output}");
        assert_eq!(output.matches("\"size\":100}").count(), 1, "{output}");
        assert_eq!(output.matches("\"size\":1000}").count(), 1, "{output}");
    }

    #[test]
    fn varying_settings_and_metrics_stay_with_their_measurements() {
        let output = comparison_at_width(&[row("reference", 0), row("solver-β", 1)], 80);
        let first = output.find("reference").unwrap();
        let second = output.find("solver-β").unwrap();
        for (section, value) in [(&output[first..second], 0), (&output[second..], 1)] {
            let compact = section.split_whitespace().collect::<String>();
            assert!(
                compact.contains(&format!("parameters:{{\"bias\":{value}}}")),
                "{section}"
            );
            assert!(
                compact.contains(&format!("metrics:{{\"absolute_error\":{value}}}")),
                "{section}"
            );
            assert!(section.contains("2.041"), "{section}");
        }
        assert_eq!(output.matches("dataset parameters:").count(), 1);
        assert!(!output.contains("metrics (all rows)"), "{output}");
    }

    #[test]
    fn table_columns_align_with_unicode_and_wrapped_values() {
        for width in [60, 110] {
            let mut table = element!(View(width:100pct,flex_direction:FlexDirection::Column) {
                #(vec![
                    table_row(vec![cell("Method",3.0,false),cell("Metrics",3.0,false),cell("Wall (ms)",2.0,true)],true),
                    table_row(vec![cell("β solver 界",3.0,false),cell(r#"{"absolute_error":0,"iterations":10000}"#,3.0,false),cell("125.011",2.0,true)],false),
                    table_row(vec![cell("reference",3.0,false),cell("{}",3.0,false),cell("2.041",2.0,true)],false),
                ])
            });
            let canvas = table.render(Some(width));
            let separators = (0..canvas.height())
                .filter_map(|y| {
                    let columns = (0..canvas.width())
                        .filter(|&x| canvas.cell(x, y).and_then(|cell| cell.text()) == Some("|"))
                        .collect::<Vec<_>>();
                    (!columns.is_empty()).then_some(columns)
                })
                .collect::<Vec<_>>();
            assert_eq!(separators.len(), 3, "{canvas}");
            assert!(
                separators.windows(2).all(|pair| pair[0] == pair[1]),
                "{canvas}"
            );
            assert_eq!(canvas.width(), width, "{canvas}");
            let text = canvas.to_string();
            assert!(text.contains("125.011") && text.contains("2.041"), "{text}");
            assert!(text.contains("β solver 界"), "{text}");
            assert!(text.contains("iterations"), "{text}");
        }
    }
}
