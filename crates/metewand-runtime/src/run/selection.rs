use std::collections::BTreeSet;

use metewand_core::{canonical::MAX_SAFE_INTEGER, planning::LogicalPlan, records::AttemptSlotRole};

use super::{
    RunError, RunOptions,
    resolution::{ResolvedBatch, ResolvedRun, canonical},
};

pub(super) fn preselect(plan: &LogicalPlan, options: &RunOptions) -> Result<LogicalPlan, RunError> {
    if options
        .max_attempts
        .is_some_and(|bound| bound == 0 || bound > MAX_SAFE_INTEGER)
    {
        return Err(RunError::Selection(
            "maximum attempts must be a positive safe integer".into(),
        ));
    }
    if options.selected_ids.is_empty() {
        return Ok(plan.clone());
    }
    for id in &options.selected_ids {
        let public = id.strip_prefix("run-store-v1:unconditional:").unwrap_or(id);
        let Some((kind, digest)) = public.rsplit_once('-') else {
            return Err(RunError::Selection(format!(
                "invalid selected identity `{id}`"
            )));
        };
        if ![
            "mw1-logical-candidate",
            "mw1-logical-specification",
            "mw1-logical-observation-slot",
            "mw1-logical-attempt-slot",
            "mw1-resolved-specification",
            "mw1-resolved-observation-slot",
            "mw1-resolved-attempt-slot",
            "mw1-attempt",
            "mw1-observation",
        ]
        .contains(&kind)
            || digest.len() != 64
            || !digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(RunError::Selection(format!(
                "invalid selected identity `{id}`"
            )));
        }
    }
    let logical = options
        .selected_ids
        .iter()
        .filter(|id| {
            [
                "mw1-logical-candidate-",
                "mw1-logical-specification-",
                "mw1-logical-observation-slot-",
                "mw1-logical-attempt-slot-",
            ]
            .iter()
            .any(|prefix| id.starts_with(prefix))
        })
        .collect::<BTreeSet<_>>();
    // Resolved identities require acquisition before their dependencies are known.
    if options.selected_ids.iter().any(|id| !logical.contains(id)) {
        return Ok(plan.clone());
    }
    let mut matched = BTreeSet::new();
    let mut selected = plan.clone();
    for experiment in &mut selected.experiments {
        for candidate in &mut experiment.candidates {
            for specification in &mut candidate.specifications {
                let mut observations = BTreeSet::new();
                specification.attempt_slots.retain(|slot| {
                    let AttemptSlotRole::Measured { observation_slot } = slot.record.role else {
                        return true;
                    };
                    let ids = [
                        candidate.candidate.id.to_string(),
                        specification.specification.id.to_string(),
                        slot.id.to_string(),
                        observation_slot.to_string(),
                    ];
                    let selected = logical
                        .iter()
                        .filter(|id| ids.contains(id))
                        .collect::<Vec<_>>();
                    for id in &selected {
                        matched.insert((**id).clone());
                    }
                    if !selected.is_empty() {
                        observations.insert(observation_slot);
                    }
                    !selected.is_empty()
                });
                specification
                    .observation_slots
                    .retain(|slot| observations.contains(&slot.id));
            }
            candidate
                .specifications
                .retain(|specification| !specification.observation_slots.is_empty());
        }
        experiment
            .candidates
            .retain(|candidate| !candidate.specifications.is_empty());
    }
    selected
        .experiments
        .retain(|experiment| !experiment.candidates.is_empty());
    for id in logical {
        if !matched.contains(id) {
            return Err(RunError::Selection(format!(
                "selected identity `{id}` matches no measured slot"
            )));
        }
    }
    Ok(selected)
}

pub(super) fn rebuild_batches(
    resolved: &mut ResolvedRun,
    schedule: Vec<(usize, usize)>,
) -> Result<(), RunError> {
    let mut batches = Vec::new();
    let mut start = 0;
    while start < schedule.len() {
        let candidate = &resolved.candidates[schedule[start].0];
        let mut end = start + 1;
        while end < schedule.len()
            && resolved.candidates[schedule[end].0].experiment == candidate.experiment
        {
            end += 1;
        }
        let bound = candidate.reuse_key.policy.max_concurrency;
        let lanes = bound.min((end - start) as u64) as usize;
        for (batch_index, batch_start) in (start..end).step_by(lanes).enumerate() {
            let batch_end = (batch_start + lanes).min(end);
            for (lane, &(candidate, measurement)) in
                schedule[batch_start..batch_end].iter().enumerate()
            {
                let provenance =
                    &mut resolved.candidates[candidate].measurements[measurement].provenance;
                let mut value = provenance.as_json().clone();
                value["concurrency"] = serde_json::json!({"max_concurrency": bound, "dispatch": "fixed_batches", "batch_index": batch_index, "batch_size": batch_end - batch_start, "lane_index": lane});
                *provenance = canonical(value)?;
            }
            batches.push(ResolvedBatch {
                slots: batch_start..batch_end,
                lane_count: lanes,
            });
        }
        start = end;
    }
    resolved.schedule = schedule;
    resolved.batches = batches;
    Ok(())
}
