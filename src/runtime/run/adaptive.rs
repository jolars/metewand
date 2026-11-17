//! Complete growing pilot rounds before freezing any measured solver budget.

use super::*;
use crate::core::{
    adaptive::{
        AdaptiveBudget, AdaptiveBudgetDecision, AdaptiveBudgetGroupResource, CalibrationPilot,
    },
    duration::positive_nanoseconds,
    identity::{canonical_identity_bytes, identify_canonical, identify_record},
};
use std::collections::{BTreeMap, BTreeSet};

struct Group {
    id: String,
    candidates: Vec<usize>,
    budget: AdaptiveBudget,
    ceiling: Option<u64>,
}

fn groups(candidates: &[ResolvedCandidate]) -> Result<Vec<Group>, RunError> {
    let mut groups: Vec<Group> = Vec::new();
    let mut keys = BTreeMap::new();
    for (index, candidate) in candidates.iter().enumerate() {
        let Some(profile) = &candidate.profile else {
            continue;
        };
        let Some(budget) = &profile.policy.adaptive_budget else {
            continue;
        };
        let Some(measurement) = candidate.measurements.first() else {
            continue;
        };
        let key = canonical(json!({
            "experiment":candidate.experiment,
            "problem_instance":measurement.specification.record.problem_instance,
            "policy":profile.policy,
            "execution_policy":candidate.measurements[0].provenance.as_json()["execution_policy"],
        }))?
        .to_canonical_bytes()?;
        let group_index = *keys.entry(key).or_insert_with(|| {
            groups.push(Group {
                id: String::new(),
                candidates: Vec::new(),
                budget: budget.clone(),
                ceiling: profile
                    .policy
                    .active_time_bound_ns()
                    .expect("validated policy"),
            });
            groups.len() - 1
        });
        let group = &mut groups[group_index];
        if let Some(&first) = group.candidates.first() {
            let reference = candidates[first].profile.as_ref().expect("buffered");
            if reference.instrumented_scope != profile.instrumented_scope
                || reference.scope != profile.scope
            {
                return Err(RunError::Unsupported("adaptive budgets require the same named instrumented scope and clock semantics for all methods".into()));
            }
        }
        group.candidates.push(index);
    }
    for group in &mut groups {
        let mut members = group
            .candidates
            .iter()
            .map(|&i| {
                let mut specification = candidates[i].measurements[0].specification.record.clone();
                specification.adaptive_budget_group = None;
                identify_record(specification).map(|s| s.id.to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        members.sort();
        let profile = candidates[group.candidates[0]]
            .profile
            .as_ref()
            .expect("buffered");
        group.id = identify_canonical::<AdaptiveBudgetGroupResource>(&canonical(
            json!({"members":members,"policy":profile.policy,"scope":profile.instrumented_scope}),
        )?)?
        .to_string();
    }
    Ok(groups)
}

pub(super) fn bind_groups(candidates: &mut [ResolvedCandidate]) -> Result<(), RunError> {
    for group in groups(candidates)? {
        let id = group.id.parse().expect("typed group identity");
        for index in group.candidates {
            for measurement in &mut candidates[index].measurements {
                measurement.specification.record.adaptive_budget_group = Some(id);
                rebind_measurement(measurement)?;
            }
        }
    }
    Ok(())
}

/// Returns newly allocated pilot outcomes. Unfinished groups keep
/// their adaptive declaration and cannot enter the measured schedule.
pub(super) fn calibrate(
    repository: &CheckedRepository,
    resolved: &mut resolution::ResolvedRun,
    store: &mut RunStore,
    options: &RunOptions,
    selected: &BTreeSet<(usize, usize)>,
    progress: &mut impl FnMut(&RunProgress) -> io::Result<()>,
) -> Result<
    (
        Vec<AdaptiveBudgetDecision>,
        Vec<crate::core::run_output::CalibrationAttemptOutput>,
    ),
    RunError,
> {
    // Validate complete cohorts before allocating any pilot slots.
    let groups = groups(&resolved.candidates)?;
    let mut decisions = Vec::new();
    let mut attempts = Vec::new();
    let mut verified_saved = false;
    for group in groups {
        if !selected
            .iter()
            .any(|(index, _)| group.candidates.contains(index))
        {
            continue;
        }
        let saved = store.adaptive_decision(&group.id)?;
        let decision = if let Some(decision) = saved {
            if !verified_saved {
                let mut inspection = crate::runtime::inspection::Inspection::default();
                inspection
                    .add_store(store.root())
                    .map_err(|e| io::Error::other(e.to_string()))?;
                verified_saved = true;
            }
            Some(decision)
        } else {
            calibrate_group(
                repository,
                resolved,
                store,
                &group,
                options,
                &mut attempts,
                progress,
            )?
        };
        if let Some(decision) = decision {
            for &index in &group.candidates {
                let candidate = &mut resolved.candidates[index];
                apply_limit(candidate, decision.effective_limit_ns, &group.budget)?;
                for measurement in &mut candidate.measurements {
                    let mut value = measurement.provenance.as_json().clone();
                    value["adaptive_budget"] = json!(decision);
                    record_limits(&mut value, candidate.timing, candidate.timeouts);
                    measurement.provenance = canonical(value)?;
                }
            }
            decisions.push(decision);
        }
    }
    Ok((decisions, attempts))
}

fn calibrate_group(
    repository: &CheckedRepository,
    resolved: &mut resolution::ResolvedRun,
    store: &mut RunStore,
    group: &Group,
    options: &RunOptions,
    attempts: &mut Vec<crate::core::run_output::CalibrationAttemptOutput>,
    progress: &mut impl FnMut(&RunProgress) -> io::Result<()>,
) -> Result<Option<AdaptiveBudgetDecision>, RunError> {
    let mut pilots = Vec::new();
    let mut limit = group
        .budget
        .initial_limit(group.ceiling)
        .map_err(RunError::Unsupported)?;
    let mut round = 0;
    loop {
        let coordinate = CalibrationPilot {
            group_id: group.id.clone(),
            round,
            limit_ns: limit,
        };
        let mut complete = true;
        let mut round_pilots = Vec::new();
        for &index in &group.candidates {
            let candidate = &mut resolved.candidates[index];
            let pilot = pilot_measurement(candidate, &coordinate)?;
            let implementation = pilot.provenance.as_json()["comparison"]["implementation"]
                .as_str()
                .expect("resolved implementation name")
                .to_owned();
            store.register_buffered(
                pilot.candidate,
                &pilot.specification,
                &pilot.attempt_slot,
                &pilot.observation_slots,
            )?;
            let evidence = store.pilot_evidence(
                pilot.attempt_slot.id,
                pilot.candidate.to_string(),
                &implementation,
                &coordinate,
            )?;
            if let Some(evidence) = evidence {
                round_pilots.push(evidence);
                continue;
            }
            if store.slot_progress(pilot.attempt_slot.id)?.active
                || options
                    .max_attempts
                    .is_some_and(|n| attempts.len() as u64 >= n)
            {
                complete = false;
                continue;
            }
            progress(&RunProgress::Preparing(PreparationProgress {
                phase: PreparationPhase::CalibratingBudget,
                component: format!(
                    "{}: pilot round {}, {} ms",
                    pilot.provenance.as_json()["comparison"]["implementation"]
                        .as_str()
                        .unwrap_or("solver"),
                    round + 1,
                    limit / 1_000_000
                ),
            }))?;
            let Some(mut execution) = AttemptExecution::reserve(store, &pilot, false)? else {
                complete = false;
                continue;
            };
            let saved = (
                candidate.profile.clone(),
                candidate.timing,
                candidate.timeouts,
                candidate.reuse_key.policy.clone(),
            );
            apply_limit(candidate, limit, &group.budget)?;
            let mut pilot = pilot;
            let mut value = pilot.provenance.as_json().clone();
            record_limits(&mut value, candidate.timing, candidate.timeouts);
            pilot.provenance = canonical(value)?;
            let result = profile::execute(
                repository,
                candidate,
                &pilot,
                resolved.private_root.path(),
                &mut Default::default(),
                &mut execution,
            );
            (
                candidate.profile,
                candidate.timing,
                candidate.timeouts,
                candidate.reuse_key.policy,
            ) = saved;
            attempts.push(crate::core::run_output::CalibrationAttemptOutput {
                pilot: coordinate.clone(),
                attempt: result?,
            });
            let evidence = store.pilot_evidence(
                pilot.attempt_slot.id,
                pilot.candidate.to_string(),
                &implementation,
                &coordinate,
            )?;
            if let Some(evidence) = evidence {
                round_pilots.push(evidence);
            } else {
                complete = false;
            }
        }
        if !complete {
            return Ok(None);
        }
        let reference = round_pilots
            .iter()
            .filter(|p| p.convergence_time_ns.is_some())
            .min_by_key(|p| (p.convergence_time_ns, p.candidate_id.clone()))
            .cloned();
        if reference.is_none() && !round_pilots.iter().any(|pilot| pilot.was_time_limited()) {
            let mut inspection = crate::runtime::inspection::Inspection::default();
            inspection
                .add_store(store.root())
                .map_err(|e| io::Error::other(e.to_string()))?;
            return Err(RunError::AdaptiveCalibrationStopped {
                group_id: group.id.clone(),
                pilots: round_pilots,
            });
        }
        pilots.extend(round_pilots);
        if reference.is_none()
            && let Some(next) = group
                .budget
                .next_limit(limit, group.ceiling)
                .map_err(RunError::Unsupported)?
        {
            limit = next;
            round += 1;
            continue;
        }
        // Verify the retained artifacts before using their accepted timing.
        let mut inspection = crate::runtime::inspection::Inspection::default();
        inspection
            .add_store(store.root())
            .map_err(|e| io::Error::other(e.to_string()))?;
        let reference_time = reference.as_ref().and_then(|p| p.convergence_time_ns);
        let decision = AdaptiveBudgetDecision {
            schema_version: 1,
            group_id: group.id.clone(),
            timing_basis: "instrumented".into(),
            instrumented_scope: resolved.candidates[group.candidates[0]]
                .profile
                .as_ref()
                .expect("buffered")
                .instrumented_scope
                .clone(),
            multiplier: group.budget.multiplier,
            minimum_limit_ns: positive_nanoseconds(&group.budget.minimum_time)
                .map_err(RunError::Unsupported)?,
            ceiling_limit_ns: group.ceiling,
            effective_limit_ns: group
                .budget
                .effective_limit(reference_time, group.ceiling)
                .map_err(RunError::Unsupported)?,
            reference_attempt_id: reference.as_ref().map(|p| p.attempt_id.clone()),
            reference_observation_id: reference.as_ref().and_then(|p| p.observation_id.clone()),
            reference_time_ns: reference_time,
            fallback: reference.is_none(),
            pilots,
        };
        return Ok(Some(
            store.freeze_adaptive_decision(&decision, attempt::now())?,
        ));
    }
}

fn apply_limit(
    candidate: &mut ResolvedCandidate,
    limit: u64,
    budget: &AdaptiveBudget,
) -> Result<(), RunError> {
    let profile = candidate.profile.as_mut().expect("buffered");
    profile.policy.max_active_time = format!("{} ms", limit / 1_000_000);
    profile.policy.adaptive_budget = None;
    let watchdog = std::time::Duration::from_nanos(
        limit + positive_nanoseconds(&budget.watchdog_grace).map_err(RunError::Unsupported)?,
    );
    candidate.timing.timeout = Some(watchdog);
    candidate.timeouts.execute = candidate.timeouts.execute.max(watchdog);
    candidate.reuse_key.policy.timeout = Some(format!("{} ms", watchdog.as_millis()));
    Ok(())
}

fn record_limits(
    value: &mut Value,
    timing: ExecutionTimingPolicy,
    timeouts: crate::runtime::worker_session::PhaseTimeouts,
) {
    value["scientific_timeout_ns"] = json!(
        timing
            .timeout
            .expect("adaptive watchdog")
            .as_nanos()
            .to_string()
    );
    value["operational_timeouts_ns"] = timeouts.provenance();
}

fn pilot_measurement(
    candidate: &ResolvedCandidate,
    coordinate: &CalibrationPilot,
) -> Result<ResolvedMeasurement, RunError> {
    let mut pilot = candidate.measurements[0].clone();
    pilot.specification.record.calibration = Some(coordinate.clone());
    rebind_measurement(&mut pilot)?;
    let mut value = pilot.provenance.as_json().clone();
    value["calibration"] = json!(coordinate);
    pilot.provenance = canonical(value)?;
    Ok(pilot)
}

fn rebind_measurement(measurement: &mut ResolvedMeasurement) -> Result<(), RunError> {
    measurement.specification = identify_record(measurement.specification.record.clone())?;
    measurement.attempt_slot.record.specification = measurement.specification.id;
    measurement.attempt_slot = identify_record(measurement.attempt_slot.record.clone())?;
    for observation in &mut measurement.observation_slots {
        observation.record.specification = measurement.specification.id;
        *observation = identify_record(observation.record.clone())?;
    }
    measurement.observation_slot = measurement.observation_slots[0].clone();
    for warmup in &mut measurement.warmups {
        warmup.record.specification = measurement.specification.id;
        *warmup = identify_record(warmup.record.clone())?;
    }
    measurement.selection_ids.extend([
        measurement.specification.id.to_string(),
        measurement.attempt_slot.id.to_string(),
    ]);
    measurement.selection_ids.extend(
        measurement
            .observation_slots
            .iter()
            .map(|o| o.id.to_string()),
    );
    let mut value = measurement.provenance.as_json().clone();
    for (key, bytes) in [
        (
            "resolved_specification",
            canonical_identity_bytes(&measurement.specification.record)?,
        ),
        (
            "resolved_attempt_slot",
            canonical_identity_bytes(&measurement.attempt_slot.record)?,
        ),
        (
            "resolved_observation_slot",
            canonical_identity_bytes(&measurement.observation_slot.record)?,
        ),
    ] {
        value[key] = serde_json::from_slice(&bytes).expect("canonical identity");
    }
    measurement.provenance = canonical(value)?;
    Ok(())
}
