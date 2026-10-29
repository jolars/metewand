use super::*;
use crate::{resource_accounting::TreeMonitor, worker_session::StreamMessage};
use metewand_core::{
    measurements::STREAMING_MEASUREMENTS_VERSION,
    profile::{
        ExecutorBasis, SelectedStreamingProfile, StopReason, StreamingDelivery,
        StreamingObservationRecord,
    },
    records::CompletionStatus,
};
use metewand_protocol::{ExecuteResponse, StreamAction};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
struct ActiveClock {
    origin: Option<Instant>,
    segment: Option<Instant>,
    active: Duration,
}
impl ActiveClock {
    fn resume(&mut self) {
        self.resume_at(Instant::now());
    }
    fn resume_at(&mut self, now: Instant) {
        self.origin.get_or_insert(now);
        self.segment = Some(now);
    }
    fn pause(&mut self) {
        self.pause_at(Instant::now());
    }
    fn pause_at(&mut self, now: Instant) {
        if let Some(start) = self.segment.take() {
            self.active += now.duration_since(start);
        }
    }
    fn used(&self) -> Duration {
        self.active + self.segment.map_or(Duration::ZERO, |start| start.elapsed())
    }
    fn deadline(&self, limit: Duration) -> Result<Instant, FixedDatasetRunError> {
        Instant::now()
            .checked_add(limit.saturating_sub(self.used()))
            .ok_or(FixedDatasetRunError::InvalidScientificTimeout)
    }
    fn update(&self, measurements: &mut ExecutionMeasurements, completed: bool) {
        if self.origin.is_none() {
            let missing =
                Measurement::unavailable(metewand_core::measurements::MissingReason::NotStarted);
            measurements.timed_wall_time_ns = missing;
            measurements.elapsed_wall_time_ns = Some(missing);
            measurements.paused_time_ns = Some(missing);
            return;
        }
        let elapsed = self.origin.map_or(Duration::ZERO, |start| start.elapsed());
        measurements.timed_wall_time_ns = Measurement::elapsed(self.used(), completed);
        measurements.elapsed_wall_time_ns = Some(Measurement::elapsed(elapsed, completed));
        measurements.paused_time_ns = Some(Measurement::elapsed(
            elapsed.saturating_sub(self.used()),
            completed,
        ));
    }
}

pub(crate) fn run_streaming(
    request: FixedDatasetRun<'_>,
    profile: &SelectedStreamingProfile,
    view: &DatasetView,
    measurements: &mut ExecutionMeasurements,
    mut session: Option<&mut WorkerSession>,
    event: &mut dyn FnMut(ProfileEvent<'_>) -> Result<(), FixedDatasetRunError>,
) -> Result<(), FixedDatasetRunError> {
    let limit = Duration::from_nanos(
        profile
            .policy
            .active_time_ns()
            .map_err(FixedDatasetRunError::InvalidStream)?,
    );
    let limit = request
        .timing
        .timeout
        .map_or(limit, |other| limit.min(other));
    if limit.is_zero() {
        return Err(FixedDatasetRunError::InvalidScientificTimeout);
    }
    let integrity = DatasetIntegrity {
        directory: request.dataset_dir,
        digest: view.digest(),
        view: Some(view),
    };
    integrity.verify("stream start")?;
    let started_at = SystemTime::now();
    let mut clock = ActiveClock::default();
    let mut tree = None;
    let mut owned = None;
    let mut evaluator: Option<WorkerSession> = None;
    measurements.version = STREAMING_MEASUREMENTS_VERSION;
    measurements.cpu_accounting = Some("unavailable".into());
    measurements.cpu_time_scope = match request.timing.scope {
        TimingScope::ColdEndToEnd => AccountingScope::WorkerLifetimeThroughExecute,
        TimingScope::PrepareAndExecute => AccountingScope::PrepareAndExecute,
        TimingScope::ExecuteOnly => AccountingScope::ExecuteOnly,
    };
    let result = (|| {
        let dataset_dir = path_string(request.dataset_dir)?;
        let validator = WorkerOutputValidator::new(request.schemas)?;
        let result_root = private_directory(request.private_root, "stream-")?;
        if session.is_none() {
            if request.timing.scope == TimingScope::ColdEndToEnd {
                clock.resume();
            }
            owned = Some(measure(&mut measurements.worker_startup_time_ns, || {
                let process = launch_trusted_local_worker(
                    request.implementation_launch,
                    request.private_root,
                    request.log_limits,
                )
                .map_err(|source| FixedDatasetRunError::ImplementationLaunch { source })?;
                WorkerSession::connect_with_deadline(
                    process,
                    WorkerRole::Implementation,
                    request.implementation_identity.clone(),
                    &[Capability::StreamingProfile],
                    request.timeouts,
                    clock.origin.map(|_| clock.deadline(limit)).transpose()?,
                )
                .map_err(|source| FixedDatasetRunError::Implementation { source })
            })?);
        } else {
            measurements.worker_startup_time_ns = Measurement::elapsed(Duration::ZERO, true);
        }
        let implementation = session
            .as_deref_mut()
            .or(owned.as_mut())
            .expect("implementation session");
        tree = Some(
            TreeMonitor::new(implementation.tree_source())
                .map_err(FixedDatasetRunError::PrivateDirectory)?,
        );
        let accounting = tree.as_ref().expect("accounting");
        measurements.cpu_accounting = Some(accounting.name().into());
        if request.timing.scope == TimingScope::ColdEndToEnd {
            accounting.begin_from_launch();
        }
        event(ProfileEvent::Phase(
            crate::run_store::AttemptPhase::Preparing,
        ))?;
        if request.timing.scope == TimingScope::PrepareAndExecute {
            clock.resume();
            accounting.resume();
        }
        implementation
            .set_scientific_deadline(clock.origin.map(|_| clock.deadline(limit)).transpose()?);
        integrity.operation("stream prepare", || {
            measure(&mut measurements.prepare_time_ns, || {
                implementation
                    .prepare(
                        dataset_dir.clone(),
                        request.problem_contract_id,
                        request.problem_parameters.clone(),
                        request.implementation_parameters.clone(),
                        request.implementation_seed,
                    )
                    .map_err(|source| FixedDatasetRunError::Implementation { source })
            })
        })?;
        event(ProfileEvent::Phase(
            crate::run_store::AttemptPhase::Executing,
        ))?;
        if request.timing.scope == TimingScope::ExecuteOnly {
            clock.resume();
            accounting.resume();
        }
        implementation.set_scientific_deadline(Some(clock.deadline(limit)?));
        let execute_start = Instant::now();
        let first = implementation.start_stream(
            path_string(result_root.path())?,
            serde_json::to_value(profile).expect("selected stream"),
        );
        let mut execute_time = execute_start.elapsed();
        clock.pause();
        measurements.cpu_time_ns = accounting.pause();
        measurements.execute_request_time_ns = Measurement::elapsed(execute_time, first.is_ok());
        let mut message =
            first.map_err(|source| FixedDatasetRunError::Implementation { source })?;
        let mut previous_index = None;
        let mut previous_work = BTreeMap::new();
        let mut directories = BTreeSet::new();
        let mut crossed = Vec::new();
        let mut evaluation_total = Duration::ZERO;
        let mut stop_reason = StopReason::ImplementationStopped;
        loop {
            implementation.set_scientific_deadline(None);
            let checkpoint = match message {
                StreamMessage::Finished(_) => break,
                StreamMessage::Checkpoint(checkpoint) => checkpoint,
            };
            clock.update(measurements, true);
            measurements.peak_rss_bytes = implementation.process_usage().since_optional(None).1;
            if previous_index.is_some_and(|n| checkpoint.check_index <= n)
                || (!checkpoint.terminal
                    && !profile.check_schedule.includes(checkpoint.check_index))
                || checkpoint
                    .work
                    .iter()
                    .any(|(k, v)| previous_work.get(k).is_some_and(|last| v < last))
            {
                return Err(FixedDatasetRunError::InvalidStream(
                    "checkpoint progress violates its schedule or decreases".into(),
                ));
            }
            previous_index = Some(checkpoint.check_index);
            previous_work.extend(checkpoint.work.clone());
            event(ProfileEvent::Phase(
                crate::run_store::AttemptPhase::Checkpointing,
            ))?;
            let (directory, manifest) = crate::streaming::snapshot_directory(
                result_root.path(),
                checkpoint
                    .manifest
                    .as_deref()
                    .expect("finalized checkpoint"),
                profile.policy.max_checkpoint_bytes,
            )
            .map_err(|e| FixedDatasetRunError::InvalidStream(e.to_string()))?;
            if !directories.insert(directory.clone()) {
                return Err(FixedDatasetRunError::InvalidStream(
                    "checkpoint directories cannot be reused".into(),
                ));
            }
            let digest = hash_local_tree(&directory)
                .map_err(|source| FixedDatasetRunError::ResultTreeHash { source })?;
            let result_output = private_directory(request.private_root, "stream-snapshot-")?;
            crate::publication::copy_prepared(&directory, result_output.path())
                .map_err(FixedDatasetRunError::PrivateDirectory)?;
            verify_result_tree(&directory, digest)?;
            verify_result_tree(result_output.path(), digest)?;
            event(ProfileEvent::Phase(
                crate::run_store::AttemptPhase::Validating,
            ))?;
            let response =
                ExecuteResponse::new(checkpoint.id.clone(), None, manifest, Default::default());
            let validated_result = validator
                .validate_execution_result(result_output.path(), &response, request.result_schema)
                .map_err(|source| FixedDatasetRunError::InvalidResult { source })?;
            if evaluator.is_none() {
                evaluator = Some(measure(
                    &mut measurements.evaluator_startup_time_ns,
                    || {
                        let process = launch_trusted_local_worker(
                            request.evaluator_launch,
                            request.private_root,
                            request.log_limits,
                        )
                        .map_err(|source| FixedDatasetRunError::EvaluatorLaunch { source })?;
                        WorkerSession::connect(
                            process,
                            WorkerRole::ProblemEvaluator,
                            request.evaluator_identity.clone(),
                            &[],
                            request.timeouts,
                        )
                        .map_err(|source| FixedDatasetRunError::Evaluator { source })
                    },
                )?);
            }
            let evaluator = evaluator.as_mut().expect("evaluator");
            event(ProfileEvent::Phase(
                crate::run_store::AttemptPhase::Evaluating,
            ))?;
            let metrics_output = private_directory(request.private_root, "stream-metrics-")?;
            let metrics_path = metrics_output.path().join("metrics.json");
            let evaluation_start = Instant::now();
            let evaluated = integrity.operation("stream evaluation", || {
                evaluator
                    .evaluate(
                        dataset_dir.clone(),
                        request.problem_contract_id,
                        request.problem_parameters.clone(),
                        request.problem_semantics.clone(),
                        path_string(result_output.path())?,
                        path_string(&metrics_path)?,
                    )
                    .map_err(|source| FixedDatasetRunError::Evaluator { source })
            });
            let evaluation_time = evaluation_start.elapsed();
            evaluation_total += evaluation_time;
            measurements.evaluation_time_ns =
                Measurement::elapsed(evaluation_total, evaluated.is_ok());
            let evaluated = evaluated?;
            verify_result_tree(result_output.path(), digest)?;
            verify_result_tree(&directory, digest)?;
            if evaluated.verdict() == EvaluationVerdict::Invalid {
                return Err(FixedDatasetRunError::ScientificInvalidResult);
            }
            let validated_metrics = validator
                .validate_metrics(
                    metrics_output.path(),
                    Path::new("metrics.json"),
                    request.metric_schema,
                )
                .map_err(|source| FixedDatasetRunError::InvalidMetrics { source })?;
            let metric = profile
                .policy
                .metric
                .as_ref()
                .map(|name| {
                    validated_metrics
                        .data()
                        .as_json()
                        .get(name)
                        .and_then(Value::as_f64)
                        .filter(|v| v.is_finite())
                        .ok_or_else(|| {
                            FixedDatasetRunError::InvalidStream(
                                "independent target metric must be finite and numeric".into(),
                            )
                        })
                })
                .transpose()?;
            let reached = metric
                .zip(profile.policy.threshold)
                .is_some_and(|(value, target)| profile.policy.reaches(value, target));
            let completion = if reached {
                CompletionStatus::Reached
            } else if metric.is_some() {
                CompletionStatus::NotReached
            } else {
                CompletionStatus::NotRequired
            };
            let reason = if reached {
                Some(StopReason::TargetReached)
            } else if checkpoint.sequence + 1 == profile.policy.max_observations {
                Some(if metric.is_none() {
                    StopReason::FixedCount
                } else {
                    StopReason::ObservationLimit
                })
            } else if clock.used() >= limit {
                Some(StopReason::ActiveTimeLimit)
            } else if checkpoint.terminal {
                Some(StopReason::ImplementationStopped)
            } else {
                None
            };
            let thresholds: Vec<_> = profile
                .policy
                .thresholds()
                .into_iter()
                .filter(|t| {
                    !crossed.contains(t) && metric.is_some_and(|v| profile.policy.reaches(v, *t))
                })
                .collect();
            crossed.extend(&thresholds);
            let coordinate = StreamingObservationRecord {
                delivery: StreamingDelivery::StreamingProfile,
                measurement_basis: ExecutorBasis::ExecutorObserved,
                checkpoint_sequence: checkpoint.sequence,
                check_index: checkpoint.check_index,
                work: checkpoint.work,
                terminal: checkpoint.terminal,
                thresholds,
                active_time_ns: clock
                    .used()
                    .as_nanos()
                    .try_into()
                    .map_err(|_| FixedDatasetRunError::InvalidScientificTimeout)?,
                elapsed_wall_time_ns: clock
                    .origin
                    .expect("started clock")
                    .elapsed()
                    .as_nanos()
                    .try_into()
                    .map_err(|_| FixedDatasetRunError::InvalidScientificTimeout)?,
                evaluation_time_ns: evaluation_total
                    .as_nanos()
                    .try_into()
                    .map_err(|_| FixedDatasetRunError::InvalidScientificTimeout)?,
                decision: if reason.is_some() { "stop" } else { "continue" }.into(),
                stop_reason: reason.clone(),
            };
            clock.update(measurements, true);
            let provenance = json!({"environment_class":"development","executor":"trusted_local","architecture":std::env::consts::ARCH,"os":std::env::consts::OS,
                "implementation":worker_provenance(request.implementation_launch,implementation),"evaluator":worker_provenance(request.evaluator_launch,evaluator),
                "problem_contract_id":request.problem_contract_id,"problem_parameters":request.problem_parameters,"implementation_parameters":request.implementation_parameters,
                "implementation_seed":request.implementation_seed,"timing_scope":request.timing.scope.to_string(),"timing_excluded_phases":["checkpoint_validation","evaluation","publication","stop_acknowledgment"],
                "scientific_timeout_ns":request.timing.timeout.map(|t|t.as_nanos().to_string()),"operational_timeouts_ns":request.timeouts.provenance(),
                "result_schema":request.result_schema,"metric_schema":request.metric_schema,"profile_observation":coordinate,"profile_policy":profile,
                "dataset_directory":request.dataset_dir,"dataset_tree_sha256":hex_digest(integrity.digest),"controls":metewand_core::controls::default_local_control_report()});
            let mut result = FixedDatasetResult {
                measurements: measurements.clone(),
                result: validated_result.data().clone(),
                metrics: validated_metrics.data().clone(),
                completion,
                timed_wall_time: clock.used(),
                implementation_time_ns: None,
                statistics: json!({}),
                evaluation_time,
                started_at,
                finished_at: SystemTime::now(),
                implementation_output: implementation.log_snapshot(),
                evaluator_output: evaluator.log_snapshot(),
                result_tree_hash: digest,
                metrics_bytes: fs::read(&metrics_path)
                    .map_err(FixedDatasetRunError::PrivateDirectory)?,
                execution_provenance: provenance,
                validated_result,
                validated_metrics,
                result_output,
                metrics_output,
            };
            event(ProfileEvent::Observation(&mut result))?;
            let reason_text = reason.as_ref().map(|r| {
                serde_json::to_value(r)
                    .expect("reason")
                    .as_str()
                    .expect("text")
                    .to_owned()
            });
            let stopping = reason.is_some();
            if let Some(reason) = reason {
                stop_reason = reason;
            } else {
                event(ProfileEvent::Phase(
                    crate::run_store::AttemptPhase::Executing,
                ))?;
                accounting.resume();
                clock.resume();
                implementation.set_scientific_deadline(Some(clock.deadline(limit)?));
            }
            #[cfg(test)]
            crate::fault_injection::hit("stream.decision.before_send");
            let start = Instant::now();
            let response = implementation.decide_stream(
                if stopping {
                    StreamAction::Stop
                } else {
                    StreamAction::Continue
                },
                reason_text,
            );
            if !stopping {
                execute_time += start.elapsed();
                clock.pause();
                measurements.cpu_time_ns = accounting.pause();
            }
            measurements.execute_request_time_ns =
                Measurement::elapsed(execute_time, response.is_ok());
            message = response.map_err(|source| FixedDatasetRunError::Implementation { source })?;
            #[cfg(test)]
            crate::fault_injection::hit("stream.decision.after_receive");
        }
        event(ProfileEvent::Stopped(stop_reason))?;
        Ok(())
    })();
    clock.pause();
    if let Some(tree) = &tree {
        measurements.cpu_time_ns = tree.pause();
    }
    clock.update(measurements, result.is_ok());
    if let Some(session) = session {
        session.set_scientific_deadline(None);
    }
    integrity.verify("stream finalization")?;
    result?;
    if let Some(implementation) = owned {
        integrity.operation("implementation shutdown", || {
            implementation
                .shutdown()
                .map_err(|source| FixedDatasetRunError::Implementation { source })
        })?;
    }
    if let Some(evaluator) = evaluator {
        integrity.operation("evaluator shutdown", || {
            evaluator
                .shutdown()
                .map_err(|source| FixedDatasetRunError::Evaluator { source })
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn active_segments_include_serialization_and_exclude_evaluation_pauses() {
        let origin = Instant::now();
        let mut clock = ActiveClock::default();
        clock.resume_at(origin);
        clock.pause_at(origin + Duration::from_nanos(30));
        clock.resume_at(origin + Duration::from_nanos(1000));
        clock.pause_at(origin + Duration::from_nanos(1040));
        assert_eq!(clock.active, Duration::from_nanos(70));
    }
}
