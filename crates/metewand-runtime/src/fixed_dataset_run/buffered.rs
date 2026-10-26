use super::*;
use metewand_core::{
    profile::{BufferedDelivery, InstrumentedBasis},
    profile::{BufferedManifest, SelectedBufferedProfile},
    records::BufferedObservationRecord,
};
use serde_json::json;

pub(crate) enum BufferedEvent<'a> {
    Phase(crate::run_store::AttemptPhase),
    Buffer {
        directory: &'a Path,
        manifest: &'a BufferedManifest,
    },
    Observation(&'a mut FixedDatasetResult),
}

pub(crate) fn run_buffered(
    request: FixedDatasetRun<'_>,
    profile: &SelectedBufferedProfile,
    view: &DatasetView,
    measurements: &mut ExecutionMeasurements,
    session: Option<&mut WorkerSession>,
    event: &mut dyn FnMut(BufferedEvent<'_>) -> Result<(), FixedDatasetRunError>,
) -> Result<(), FixedDatasetRunError> {
    let mut timing = TimingEnvelope::new(request.timing)?;
    let integrity = DatasetIntegrity {
        directory: request.dataset_dir,
        digest: view.digest(),
        view: Some(view),
    };
    integrity.verify("buffered attempt start")?;
    let result = run_workers(
        request,
        profile,
        &integrity,
        measurements,
        &mut timing,
        session,
        event,
    );
    if let Some(started) = timing.started
        && !timing.finished
    {
        measurements.timed_wall_time_ns = Measurement::elapsed(started.elapsed(), false);
    }
    integrity.verify("buffered attempt finalization")?;
    result
}

fn run_workers(
    request: FixedDatasetRun<'_>,
    profile: &SelectedBufferedProfile,
    integrity: &DatasetIntegrity<'_>,
    measurements: &mut ExecutionMeasurements,
    timing: &mut TimingEnvelope,
    session: Option<&mut WorkerSession>,
    event: &mut dyn FnMut(BufferedEvent<'_>) -> Result<(), FixedDatasetRunError>,
) -> Result<(), FixedDatasetRunError> {
    measurements.cpu_time_scope = match request.timing.scope {
        TimingScope::ColdEndToEnd => AccountingScope::WorkerLifetimeThroughExecute,
        TimingScope::PrepareAndExecute => AccountingScope::PrepareAndExecute,
        TimingScope::ExecuteOnly => AccountingScope::ExecuteOnly,
    };
    let started_at = SystemTime::now();
    let dataset_dir = path_string(request.dataset_dir)?;
    let validator = WorkerOutputValidator::new(request.schemas)?;

    let result_dir = private_directory(request.private_root, "result-")?;

    let result_path = path_string(result_dir.path())?;

    let mut owned_implementation = if session.is_none() {
        Some(integrity.operation("implementation hello", || {
            measure(&mut measurements.worker_startup_time_ns, || {
                if request.timing.scope == TimingScope::ColdEndToEnd {
                    timing.start()?;
                }
                let implementation_process = launch_trusted_local_worker(
                    request.implementation_launch,
                    request.private_root,
                    request.log_limits,
                )
                .map_err(|source| FixedDatasetRunError::ImplementationLaunch { source })?;
                WorkerSession::connect_with_deadline(
                    implementation_process,
                    WorkerRole::Implementation,
                    request.implementation_identity.clone(),
                    &[Capability::BufferedProfile],
                    request.timeouts,
                    timing.deadline,
                )
                .map_err(|source| FixedDatasetRunError::Implementation { source })
            })
        })?)
    } else {
        measurements.worker_startup_time_ns = Measurement::elapsed(Duration::ZERO, true);
        None
    };
    let implementation = match session {
        Some(session) => session,
        None => owned_implementation
            .as_mut()
            .expect("a fresh execution owns its session"),
    };

    event(BufferedEvent::Phase(
        crate::run_store::AttemptPhase::Preparing,
    ))?;
    let mut baseline = None;
    integrity.operation("implementation prepare", || {
        if request.timing.scope == TimingScope::PrepareAndExecute {
            baseline = Some(implementation.process_usage());
            timing.start()?;
        }
        implementation.set_scientific_deadline(timing.deadline);
        let prepared = measure(&mut measurements.prepare_time_ns, || {
            implementation
                .prepare(
                    dataset_dir.clone(),
                    request.problem_contract_id,
                    request.problem_parameters.clone(),
                    request.implementation_parameters.clone(),
                    request.implementation_seed,
                )
                .map_err(|source| FixedDatasetRunError::Implementation { source })
        });
        if timing.started.is_some() {
            let (cpu, memory) = implementation.process_usage().since_optional(baseline);
            measurements.cpu_time_ns = cpu;
            measurements.peak_rss_bytes = memory;
        }
        prepared
    })?;
    event(BufferedEvent::Phase(
        crate::run_store::AttemptPhase::Executing,
    ))?;
    let (executed, timed_wall_time) = integrity.operation("implementation execute", || {
        if request.timing.scope == TimingScope::ExecuteOnly {
            baseline = Some(implementation.process_usage());
            timing.start()?;
        }
        implementation.set_scientific_deadline(timing.deadline);
        let executed = measure(&mut measurements.execute_request_time_ns, || {
            implementation
                .execute_buffered(
                    result_path.clone(),
                    serde_json::to_value(profile).expect("selected profile"),
                )
                .map_err(|source| FixedDatasetRunError::Implementation { source })
        });
        let elapsed = if executed.is_ok() {
            match timing.finish() {
                Ok(elapsed) => Some(elapsed),
                Err(error) => {
                    implementation.discard_after_timeout();
                    return Err(error);
                }
            }
        } else {
            None
        };
        let (cpu, memory) = implementation.process_usage().since_optional(baseline);
        measurements.cpu_time_ns = cpu;
        measurements.peak_rss_bytes = memory;
        let executed = executed?;
        let elapsed = elapsed.expect("a successful exchange completed the envelope");
        measurements.timed_wall_time_ns = Measurement::elapsed(elapsed, true);
        Ok((executed, elapsed))
    })?;
    implementation.set_scientific_deadline(None);

    event(BufferedEvent::Phase(
        crate::run_store::AttemptPhase::Validating,
    ))?;
    let (manifest, trace) = crate::buffered::validate_buffer(
        result_dir.path(),
        Path::new(executed.manifest()),
        profile,
    )
    .map_err(FixedDatasetRunError::InvalidBuffer)?;
    if u128::from(manifest.instrumented_total_time_ns) > timed_wall_time.as_nanos() {
        return Err(FixedDatasetRunError::InvalidBuffer(
            "instrumented scope exceeds the executor-observed duration".into(),
        ));
    }
    let frozen = private_directory(request.private_root, "frozen-buffer-")?;
    let digest = hash_local_tree(result_dir.path())
        .map_err(|source| FixedDatasetRunError::ResultTreeHash { source })?;
    crate::publication::copy_prepared(result_dir.path(), frozen.path())
        .map_err(FixedDatasetRunError::PrivateDirectory)?;
    verify_result_tree(result_dir.path(), digest)?;
    verify_result_tree(frozen.path(), digest)?;
    // Validate every canonical snapshot before publishing the first observation.
    for snapshot in &manifest.snapshots {
        let path = &snapshot.manifest;
        let directory = frozen.path().join(path.parent().ok_or_else(|| {
            FixedDatasetRunError::InvalidBuffer("snapshot requires its own directory".into())
        })?);
        let response = metewand_protocol::ExecuteResponse::new(
            "buffer".parse().expect("static ID"),
            None,
            path.file_name()
                .and_then(|s| s.to_str())
                .ok_or(FixedDatasetRunError::NonAbsolutePath)?,
            Default::default(),
        );
        validator
            .validate_execution_result(&directory, &response, request.result_schema)
            .map_err(|source| FixedDatasetRunError::InvalidResult { source })?;
    }
    event(BufferedEvent::Buffer {
        directory: frozen.path(),
        manifest: &manifest,
    })?;
    let mut evaluator = integrity.operation("evaluator hello", || {
        measure(&mut measurements.evaluator_startup_time_ns, || {
            let evaluator_process = launch_trusted_local_worker(
                request.evaluator_launch,
                request.private_root,
                request.log_limits,
            )
            .map_err(|source| FixedDatasetRunError::EvaluatorLaunch { source })?;
            WorkerSession::connect(
                evaluator_process,
                WorkerRole::ProblemEvaluator,
                request.evaluator_identity.clone(),
                &[],
                request.timeouts,
            )
            .map_err(|source| FixedDatasetRunError::Evaluator { source })
        })
    })?;

    let base_provenance = serde_json::json!({
        "environment_class": "development",
        "executor": "trusted_local",
        "architecture": std::env::consts::ARCH,
        "os": std::env::consts::OS,
        "implementation": worker_provenance(request.implementation_launch, implementation),
        "evaluator": worker_provenance(request.evaluator_launch, &evaluator),
        "problem_contract_id": request.problem_contract_id,
        "problem_parameters": request.problem_parameters,
        "implementation_parameters": request.implementation_parameters,
        "implementation_seed": request.implementation_seed,
        "timing_scope": request.timing.scope.to_string(),
        "operation_scope": request.timing.scope == TimingScope::ExecuteOnly,
        "scientific_timeout_ns": request.timing.timeout.map(|limit| limit.as_nanos().to_string()),
        "operational_timeouts_ns": request.timeouts.provenance(),
        "timing_dataset_integrity": if request.timing.scope == TimingScope::ExecuteOnly { "outside_execute" } else { "included_between_prepare_and_execute" },
        "timing_resource_accounting": if request.timing.scope == TimingScope::ExecuteOnly { "outside_execute" } else { "included_between_prepare_and_execute" },
        "timing_coordination": if request.timing.scope == TimingScope::ExecuteOnly { "outside_execute" } else { "included_between_prepare_and_execute" },
        "timing_excluded_phases": excluded_phases(request.timing.scope),
        "dataset_directory": request.dataset_dir,
        "dataset_tree_sha256": hex_digest(integrity.digest),
        "dataset_immutability": {
            "view": if integrity.view.is_some() { "copy" } else { "caller_provided" },
            "enforcement": "best_effort",
            "verification": "each_worker_operation",
        },
        "result_schema": request.result_schema,
        "metric_schema": request.metric_schema,
        "controls": metewand_core::controls::default_local_control_report(),
    });

    event(BufferedEvent::Phase(
        crate::run_store::AttemptPhase::Evaluating,
    ))?;
    let implementation_output = implementation.log_snapshot();
    let mut evaluation_total = Duration::ZERO;
    for snapshot in &manifest.snapshots {
        let result_output = private_directory(request.private_root, "snapshot-")?;
        let snapshot_root = frozen.path().join(
            snapshot
                .manifest
                .parent()
                .expect("validated snapshot parent"),
        );
        crate::publication::copy_prepared(&snapshot_root, result_output.path())
            .map_err(FixedDatasetRunError::PrivateDirectory)?;
        let response = metewand_protocol::ExecuteResponse::new(
            "buffer".parse().expect("static ID"),
            None,
            snapshot
                .manifest
                .file_name()
                .expect("validated manifest")
                .to_str()
                .expect("UTF-8 path"),
            Default::default(),
        );
        let validated_result = validator
            .validate_execution_result(result_output.path(), &response, request.result_schema)
            .map_err(|source| FixedDatasetRunError::InvalidResult { source })?;
        let result_tree_hash = hash_local_tree(result_output.path())
            .map_err(|source| FixedDatasetRunError::ResultTreeHash { source })?;
        let metrics_output = private_directory(request.private_root, "profile-metrics-")?;
        let metrics_path = metrics_output.path().join("metrics.json");
        let evaluation_start = Instant::now();
        let evaluated = integrity.operation("buffered snapshot evaluation", || {
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
        measurements.evaluation_time_ns = Measurement::elapsed(evaluation_total, evaluated.is_ok());
        let evaluated = evaluated?;
        verify_result_tree(result_output.path(), result_tree_hash)?;
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
        let metrics = validated_metrics.data().clone();
        let completion = if let (Some(metric), Some(target)) =
            (&profile.policy.metric, profile.policy.threshold)
        {
            let value = metrics
                .as_json()
                .get(metric)
                .and_then(Value::as_f64)
                .filter(|v| v.is_finite())
                .ok_or_else(|| {
                    FixedDatasetRunError::InvalidBuffer(
                        "independent target metric must be finite and numeric".into(),
                    )
                })?;
            if profile.policy.reaches(value, target) {
                CompletionStatus::Reached
            } else {
                CompletionStatus::NotReached
            }
        } else {
            CompletionStatus::NotRequired
        };
        let point = &trace[snapshot.trace_index as usize];
        let profile_observation = BufferedObservationRecord {
            delivery: BufferedDelivery::BufferedProfile,
            measurement_basis: InstrumentedBasis::Instrumented,
            instrumented_scope: profile.instrumented_scope.clone(),
            clock: manifest.clock.clone(),
            instrumented_time_ns: point.instrumented_time_ns,
            instrumented_total_time_ns: manifest.instrumented_total_time_ns,
            trace_index: snapshot.trace_index,
            capture_ordinal: snapshot.capture_ordinal,
            thresholds: snapshot.thresholds.clone(),
            provisional_metrics: point.provisional_metrics.clone(),
        };
        let mut execution_provenance = base_provenance.clone();
        execution_provenance["profile_observation"] =
            serde_json::to_value(profile_observation).expect("typed profile");
        execution_provenance["profile_policy"] =
            serde_json::to_value(profile).expect("typed profile");
        let mut result = FixedDatasetResult {
            measurements: measurements.clone(),
            result: validated_result.data().clone(),
            metrics: metrics.clone(),
            completion,
            timed_wall_time,
            implementation_time_ns: None,
            statistics: json!({}),
            evaluation_time,
            started_at,
            finished_at: SystemTime::now(),
            implementation_output: implementation_output.clone(),
            evaluator_output: evaluator.log_snapshot(),
            result_tree_hash,
            metrics_bytes: fs::read(&metrics_path)
                .map_err(FixedDatasetRunError::PrivateDirectory)?,
            execution_provenance,
            validated_result,
            validated_metrics,
            result_output,
            metrics_output,
        };
        event(BufferedEvent::Observation(&mut result))?;
        for (name, tolerance) in &profile.scope.metric_agreement {
            let local = point
                .provisional_metrics
                .get(name)
                .and_then(Value::as_f64)
                .filter(|v| v.is_finite());
            let independent = metrics
                .as_json()
                .get(name)
                .and_then(Value::as_f64)
                .filter(|v| v.is_finite());
            if local.zip(independent).is_none_or(|(local, independent)| {
                (local - independent).abs() > tolerance.atol + tolerance.rtol * independent.abs()
            }) {
                return Err(FixedDatasetRunError::MetricDisagreement(name.clone()));
            }
        }
    }
    integrity.operation("evaluator shutdown", || {
        evaluator
            .shutdown()
            .map(|_| ())
            .map_err(|source| FixedDatasetRunError::Evaluator { source })
    })?;
    if let Some(implementation) = owned_implementation.take() {
        integrity.operation("implementation shutdown", || {
            implementation
                .shutdown()
                .map(|_| ())
                .map_err(|source| FixedDatasetRunError::Implementation { source })
        })?;
    }
    Ok(())
}
