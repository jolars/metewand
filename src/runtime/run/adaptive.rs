//! Complete growing pilot rounds before freezing any measured solver budget.

use super::*;
use crate::core::{
    adaptive::{
        AdaptiveBudget, AdaptiveBudgetDecision, AdaptiveBudgetGroupResource, AdaptiveTimingBasis,
        CalibrationPilot, PilotEvidence,
    },
    duration::positive_nanoseconds,
    identity::{identify_canonical, identify_record},
};
use crate::runtime::run_store::{
    RunStoreError, SlotDelivery, SlotRegistration, fresh::FreshPointRegistration,
};
use std::collections::BTreeMap;

struct Group {
    id: String,
    candidates: Vec<usize>,
    budget: AdaptiveBudget,
    ceiling: Option<u64>,
    basis: AdaptiveTimingBasis,
}

pub(super) fn pending(candidate: &ResolvedCandidate) -> bool {
    candidate
        .profile_policy()
        .is_some_and(|policy| policy.adaptive_budget.is_some())
}

fn timing_basis(candidate: &ResolvedCandidate) -> AdaptiveTimingBasis {
    if candidate.profile.is_some() {
        AdaptiveTimingBasis::Instrumented
    } else if candidate.streaming_profile.is_some() {
        AdaptiveTimingBasis::StreamingActiveTime
    } else {
        AdaptiveTimingBasis::CumulativeTimedWallTime
    }
}

fn groups(candidates: &[ResolvedCandidate]) -> Result<Vec<Group>, RunError> {
    let mut groups: Vec<Group> = Vec::new();
    let mut keys = BTreeMap::new();
    for (index, candidate) in candidates.iter().enumerate() {
        let Some(policy) = candidate.profile_policy() else {
            continue;
        };
        let Some(budget) = &policy.adaptive_budget else {
            continue;
        };
        let Some(measurement) = candidate.measurements.first() else {
            continue;
        };
        let key = canonical(json!({
            "experiment":candidate.experiment,
            "problem_instance":measurement.specification.record.problem_instance,
            "policy":policy,
            "execution_policy":measurement.provenance_base["execution_policy"],
        }))?
        .to_canonical_bytes()?;
        let group_index = *keys.entry(key).or_insert_with(|| {
            groups.push(Group {
                id: String::new(),
                candidates: Vec::new(),
                budget: budget.clone(),
                ceiling: policy.active_time_bound_ns().expect("validated policy"),
                basis: timing_basis(candidate),
            });
            groups.len() - 1
        });
        let group = &mut groups[group_index];
        if let Some(&first) = group.candidates.first()
            && let (Some(reference), Some(profile)) =
                (&candidates[first].profile, &candidate.profile)
            && (reference.instrumented_scope != profile.instrumented_scope
                || reference.scope != profile.scope)
        {
            return Err(RunError::Unsupported("adaptive budgets require the same named instrumented scope and clock semantics for all methods".into()));
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
        let candidate = &candidates[group.candidates[0]];
        let policy = candidate.profile_policy().expect("adaptive profile");
        let key = if let Some(profile) = &candidate.profile {
            json!({"members":members,"policy":policy,"scope":profile.instrumented_scope})
        } else {
            json!({"members":members,"policy":policy,"timing_basis":group.basis})
        };
        group.id = identify_canonical::<AdaptiveBudgetGroupResource>(&canonical(key)?)?.to_string();
    }
    Ok(groups)
}

pub(super) fn bind_groups(candidates: &mut [ResolvedCandidate]) -> Result<(), RunError> {
    for group in groups(candidates)? {
        let id = group.id.parse().expect("typed group identity");
        for index in group.candidates {
            for measurement in &mut candidates[index].measurements {
                Arc::make_mut(&mut measurement.specification)
                    .record
                    .adaptive_budget_group = Some(id);
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
    is_selected: impl Fn((usize, usize)) -> bool,
    progress: &mut impl FnMut(&RunProgress) -> io::Result<()>,
) -> Result<(Vec<AdaptiveBudgetDecision>, Vec<CalibrationAttemptOutput>), RunError> {
    // Validate complete cohorts before allocating any pilot slots.
    let groups = groups(&resolved.candidates)?;
    let mut decisions = Vec::new();
    let mut attempts = Vec::new();
    let mut verified_saved = false;
    for group in groups {
        if !group.candidates.iter().any(|&index| {
            (0..resolved.candidates[index].measurements.len())
                .any(|measurement| is_selected((index, measurement)))
        }) {
            continue;
        }
        let saved = store.adaptive_decision(&group.id)?;
        let decision = if let Some(decision) = saved {
            if !verified_saved {
                crate::runtime::inspection::Inspection::read_for_run(store.root())
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
                let mut value = candidate.measurements[0].provenance_base.as_ref().clone();
                value["adaptive_budget"] = json!(decision);
                record_limits(&mut value, candidate.timing, candidate.timeouts);
                let value = Arc::new(value);
                for measurement in &mut candidate.measurements {
                    measurement.provenance_base = Arc::clone(&value);
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
    attempts: &mut Vec<CalibrationAttemptOutput>,
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
            let saved = (
                candidate.profile.clone(),
                candidate.streaming_profile.clone(),
                candidate.fresh_profile.clone(),
                candidate.timing,
                candidate.timeouts,
                candidate.reuse_key.policy.timeout.clone(),
                candidate.reuse_key.execute_watchdog,
            );
            apply_limit(candidate, limit, &group.budget)?;
            let evidence = run_pilot(
                store,
                candidate,
                &coordinate,
                group.basis,
                |store, pilot, worker| {
                    if options
                        .max_attempts
                        .is_some_and(|n| attempts.len() as u64 >= n)
                    {
                        return Ok(());
                    }
                    let mut execution = match AttemptExecution::reserve(store, pilot, false) {
                        Ok(Some(execution)) => execution,
                        Ok(None) | Err(RunError::Store(RunStoreError::SequenceBusy)) => {
                            return Ok(());
                        }
                        Err(error) => return Err(error),
                    };
                    let mut pilot = pilot.clone();
                    let mut value = pilot.build_provenance()?.as_json().clone();
                    value["calibration"] = json!(coordinate);
                    record_limits(&mut value, candidate.timing, candidate.timeouts);
                    pilot.execution_provenance = Some(canonical(value)?);
                    progress(&RunProgress::Preparing(PreparationProgress {
                        phase: PreparationPhase::CalibratingBudget,
                        component: format!(
                            "{}: pilot round {}, {} ms",
                            pilot.provenance().as_json()["comparison"]["implementation"]
                                .as_str()
                                .unwrap_or("solver"),
                            round + 1,
                            limit / 1_000_000
                        ),
                    }))?;
                    attempts.push(CalibrationAttemptOutput {
                        pilot: coordinate.clone(),
                        attempt: execute(
                            repository,
                            candidate,
                            &pilot,
                            resolved.private_root.path(),
                            worker,
                            &mut execution,
                        )?,
                    });
                    Ok(())
                },
            );
            (
                candidate.profile,
                candidate.streaming_profile,
                candidate.fresh_profile,
                candidate.timing,
                candidate.timeouts,
                candidate.reuse_key.policy.timeout,
                candidate.reuse_key.execute_watchdog,
            ) = saved;
            let evidence = evidence?;
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
        let time_limited = round_pilots.iter().any(|pilot| {
            pilot.was_time_limited()
                && (group.basis == AdaptiveTimingBasis::Instrumented || pilot.status == "censored")
        });
        if reference.is_none() && !time_limited {
            crate::runtime::inspection::Inspection::read_for_run(store.root())
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
        crate::runtime::inspection::Inspection::read_for_run(store.root())
            .map_err(|e| io::Error::other(e.to_string()))?;
        let reference_time = reference.as_ref().and_then(|p| p.convergence_time_ns);
        let decision = AdaptiveBudgetDecision {
            schema_version: if group.basis == AdaptiveTimingBasis::Instrumented {
                1
            } else {
                2
            },
            group_id: group.id.clone(),
            timing_basis: group.basis,
            instrumented_scope: resolved.candidates[group.candidates[0]]
                .profile
                .as_ref()
                .map(|profile| profile.instrumented_scope.clone()),
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
    let watchdog =
        std::time::Duration::from_nanos(budget.watchdog(limit).map_err(RunError::Unsupported)?);
    let buffered = candidate.profile.is_some();
    let policy = candidate
        .profile
        .as_mut()
        .map(|p| &mut p.policy)
        .or_else(|| candidate.streaming_profile.as_mut().map(|p| &mut p.policy))
        .or(candidate.fresh_profile.as_mut())
        .expect("adaptive profile");
    policy.max_active_time = format!("{} ms", limit / 1_000_000);
    policy.adaptive_budget = None;
    let timeout = if buffered {
        watchdog
    } else {
        std::time::Duration::from_nanos(limit)
    };
    candidate.timing.timeout = Some(timeout);
    candidate.timeouts.execute = candidate.timeouts.execute.max(watchdog);
    candidate.reuse_key.execute_watchdog = candidate.timeouts.execute;
    candidate.reuse_key.policy.timeout = Some(format!("{} ms", timeout.as_millis()));
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

fn run_pilot(
    store: &mut RunStore,
    candidate: &ResolvedCandidate,
    coordinate: &CalibrationPilot,
    basis: AdaptiveTimingBasis,
    mut dispatch: impl FnMut(
        &RunStore,
        &ResolvedMeasurement,
        &mut crate::runtime::worker_reuse::ReusableWorker,
    ) -> Result<(), RunError>,
) -> Result<Option<PilotEvidence>, RunError> {
    let first = &candidate.measurements[0];
    let repetition = first.scheduling.comparison_block.measurement_index;
    let mut pilots = candidate
        .measurements
        .iter()
        .take_while(|point| {
            point.specification.id == first.specification.id
                && point.scheduling.comparison_block.measurement_index == repetition
        })
        .map(|measurement| {
            let mut pilot = measurement.clone();
            pilot.concurrency = None;
            Arc::make_mut(&mut pilot.specification).record.calibration = Some(coordinate.clone());
            rebind_measurement(&mut pilot)?;
            Ok(pilot)
        })
        .collect::<Result<Vec<_>, RunError>>()?;
    pilots.sort_by_key(|pilot| pilot.scheduling.observation_round);
    for batch in pilots.chunks(256) {
        let registrations = batch
            .iter()
            .map(|pilot| {
                let delivery = match basis {
                    AdaptiveTimingBasis::Instrumented => SlotDelivery::Buffered,
                    AdaptiveTimingBasis::StreamingActiveTime => SlotDelivery::Streaming,
                    AdaptiveTimingBasis::CumulativeTimedWallTime => {
                        SlotDelivery::Fresh(FreshPointRegistration {
                            measurement_index: repetition,
                            control: pilot.fresh.as_ref().expect("fresh control"),
                            policy: candidate.fresh_profile.as_ref().expect("fresh policy"),
                        })
                    }
                };
                Ok(SlotRegistration {
                    candidate: pilot.candidate,
                    specification: &pilot.specification,
                    attempt: pilot.resolve_attempt(),
                    observations: pilot.resolve_observations()?,
                    delivery,
                })
            })
            .collect::<Result<Vec<_>, RunError>>()?;
        store.register_slots(registrations)?;
    }
    if candidate.fresh_profile.is_some() {
        store.publish_fresh_repetitions(
            [(pilots[0].specification.id, repetition)],
            attempt::now(),
        )?;
    }
    let implementation = first.provenance_base["comparison"]["implementation"]
        .as_str()
        .expect("resolved implementation name");
    let mut worker = crate::runtime::worker_reuse::ReusableWorker::default();
    let last = pilots.len() - 1;
    for (index, pilot) in pilots.iter().enumerate() {
        let evidence = |store: &RunStore| {
            store.pilot_evidence(
                pilot.attempt_slot,
                pilot.candidate.to_string(),
                implementation,
                coordinate,
                basis,
            )
        };
        let outcome = match evidence(store)? {
            Some(outcome) => outcome,
            None => {
                if store.slot_progress(pilot.attempt_slot)?.active {
                    return Ok(None);
                }
                dispatch(store, pilot, &mut worker)?;
                let Some(outcome) = evidence(store)? else {
                    return Ok(None);
                };
                outcome
            }
        };
        if candidate.fresh_profile.is_some()
            && outcome.status == "failed"
            && index < last
            && store
                .slot_progress(pilot.attempt_slot)?
                .disposition
                .is_none()
        {
            continue;
        }
        if candidate.fresh_profile.is_none()
            || outcome.convergence_time_ns.is_some()
            || outcome.status != "accepted"
        {
            return Ok(Some(outcome));
        }
        if index == last {
            return Ok(Some(PilotEvidence {
                status: "censored".into(),
                reason: Some("schedule_exhausted".into()),
                ..outcome
            }));
        }
    }
    unreachable!("a pilot contains at least one attempt slot")
}

fn rebind_measurement(measurement: &mut ResolvedMeasurement) -> Result<(), RunError> {
    measurement.specification =
        Arc::new(identify_record(measurement.specification.record.clone())?);
    measurement.attempt_slot = crate::core::identity::record_id(&measurement.attempt_record())?;
    for warmup in Arc::make_mut(&mut measurement.warmups) {
        warmup.record.specification = measurement.specification.id;
        *warmup = identify_record(warmup.record.clone())?;
    }
    if !measurement.selection_ids.is_empty() {
        measurement.selection_ids.extend([
            measurement.specification.id.to_string(),
            measurement.attempt_slot.to_string(),
        ]);
        measurement.selection_ids.extend(
            measurement
                .resolve_observations()?
                .iter()
                .map(|o| o.id.to_string()),
        );
    }
    Ok(())
}
