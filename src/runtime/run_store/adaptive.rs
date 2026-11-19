//! Calibration reuses ordinary fenced attempts and insert-only coordination events.

use rusqlite::{OptionalExtension, params};

use super::{
    RunStore, RunStoreError,
    attempts::{invalid, timestamp},
};
use crate::core::{
    adaptive::{AdaptiveBudgetDecision, AdaptiveTimingBasis, CalibrationPilot, PilotEvidence},
    canonical::CanonicalValue,
    records::{RecordId, ResolvedAttemptSlotRecord},
};

pub(crate) const DECISION_EVENT: &str = "adaptive_budget_frozen";

fn decode_decision(text: &str) -> Result<AdaptiveBudgetDecision, RunStoreError> {
    let decision: AdaptiveBudgetDecision =
        serde_json::from_str(text).map_err(|_| invalid("invalid adaptive budget decision"))?;
    decision.validate_format().map_err(invalid)?;
    Ok(decision)
}

struct PilotOutcome {
    attempt_id: String,
    status: String,
    failure: Option<String>,
    directory: String,
    reason: Option<String>,
}

impl RunStore {
    pub(crate) fn adaptive_decision(
        &self,
        group_id: &str,
    ) -> Result<Option<AdaptiveBudgetDecision>, RunStoreError> {
        let text: Option<String> = self.connection.query_row(
            "SELECT payload_json FROM events WHERE kind=?1 AND json_extract(payload_json,'$.group_id')=?2 ORDER BY sequence LIMIT 1",
            params![DECISION_EVENT, group_id], |r| r.get(0)).optional()?;
        text.as_deref().map(decode_decision).transpose()
    }

    /// The immediate transaction makes the first frozen decision authoritative.
    pub(crate) fn freeze_adaptive_decision(
        &mut self,
        decision: &AdaptiveBudgetDecision,
        now_ns: u64,
    ) -> Result<AdaptiveBudgetDecision, RunStoreError> {
        decision.validate_format().map_err(invalid)?;
        #[cfg(test)]
        crate::runtime::fault_injection::hit("adaptive.freeze.before");
        let value =
            CanonicalValue::try_from(serde_json::to_value(decision).expect("typed decision"))?;
        let payload = String::from_utf8(value.to_canonical_bytes()?).expect("UTF-8");
        let tx = self.transaction()?;
        let existing: Option<String> = tx.query_row(
            "SELECT payload_json FROM events WHERE kind=?1 AND json_extract(payload_json,'$.group_id')=?2 ORDER BY sequence LIMIT 1",
            params![DECISION_EVENT, decision.group_id], |r| r.get(0)).optional()?;
        if let Some(existing) = existing {
            return decode_decision(&existing);
        }
        tx.execute("INSERT INTO events (attempt_id,occurred_at_ns,kind,payload_json) VALUES (NULL,?1,?2,?3)", params![timestamp(now_ns)?,DECISION_EVENT,payload])?;
        tx.commit()?;
        #[cfg(test)]
        crate::runtime::fault_injection::hit("adaptive.freeze.after");
        Ok(decision.clone())
    }

    pub(crate) fn pilot_evidence(
        &self,
        slot: RecordId<ResolvedAttemptSlotRecord>,
        candidate_id: String,
        implementation: &str,
        pilot: &CalibrationPilot,
        basis: AdaptiveTimingBasis,
    ) -> Result<Option<PilotEvidence>, RunStoreError> {
        // The committed transition supplies the same stop reason after resume or shared replay.
        let row = self
            .connection
            .query_row(
                "SELECT a.id,a.state,a.failure_code,coalesce(f.relative_path,''),
                    json_extract(e.payload_json,'$.reason')
             FROM attempts a
             LEFT JOIN artifacts f ON f.id=a.final_artifact_id
             JOIN attempt_transitions t ON t.attempt_id=a.id AND t.revision=a.revision
             JOIN events e ON e.sequence=t.event_sequence
             WHERE a.slot_id=?1 ORDER BY a.retry_index DESC LIMIT 1",
                [slot.to_string()],
                |row| {
                    Ok(PilotOutcome {
                        attempt_id: row.get(0)?,
                        status: row.get(1)?,
                        failure: row.get(2)?,
                        directory: row.get(3)?,
                        reason: row.get(4)?,
                    })
                },
            )
            .optional()?;
        let Some(PilotOutcome {
            attempt_id,
            mut status,
            failure,
            directory,
            mut reason,
        }) = row
        else {
            return Ok(None);
        };
        if !["accepted", "censored", "failed", "superseded"].contains(&status.as_str())
            || failure.as_deref() == Some("interrupted_attempt")
        {
            return Ok(None);
        }
        // Qualify a crossing only after the producing attempt has finalized.
        let reference: Option<(String, u64)> = if status == "accepted" {
            if basis == AdaptiveTimingBasis::CumulativeTimedWallTime {
                let elapsed = super::fresh::prefix_time(&self.connection, &slot.to_string())?;
                let observation: Option<String> = self.connection.query_row(
                    "SELECT id FROM observations WHERE producing_attempt_id=?1 AND valid=1 AND completion='reached' ORDER BY id LIMIT 1",
                    [&attempt_id], |r| r.get(0),
                ).optional()?;
                if elapsed <= pilot.limit_ns && observation.is_some() {
                    reason = Some("target_reached".into());
                    observation.map(|id| (id, elapsed))
                } else {
                    if elapsed >= pilot.limit_ns {
                        status = "censored".into();
                        reason = Some("active_time_limit".into());
                    }
                    None
                }
            } else {
                let path = if basis == AdaptiveTimingBasis::Instrumented {
                    "$.profile.instrumented_time_ns"
                } else {
                    "$.profile.active_time_ns"
                };
                self.connection.query_row(
                    "SELECT id,json_extract(record_json,?3) AS time FROM observations WHERE producing_attempt_id=?1 AND valid=1 AND completion='reached' AND time<=?2 ORDER BY time,id LIMIT 1",
                    params![attempt_id,timestamp(pilot.limit_ns)?,path], |r| Ok((r.get(0)?,r.get::<_,i64>(1)? as u64))).optional()?
            }
        } else {
            None
        };
        Ok(Some(PilotEvidence {
            implementation: Some(implementation.to_owned()),
            reason: failure.or(reason),
            round: pilot.round,
            limit_ns: pilot.limit_ns,
            candidate_id,
            attempt_id,
            attempt_directory: directory,
            status,
            convergence_time_ns: reference.as_ref().map(|(_, t)| *t),
            observation_id: reference.map(|(id, _)| id),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::Path, process::Command};

    fn decision() -> AdaptiveBudgetDecision {
        serde_json::from_slice(include_bytes!(
            "../../../schemas/v1/fixtures/adaptive-budget-v1.json"
        ))
        .unwrap()
    }

    #[test]
    fn freeze_child() {
        let Some(root) = std::env::var_os("METEWAND_ADAPTIVE_CRASH_ROOT") else {
            return;
        };
        let mut store = RunStore::open(Path::new(&root)).unwrap();
        let trace =
            fs::File::create(std::env::var_os("METEWAND_ADAPTIVE_CRASH_TRACE").unwrap()).unwrap();
        crate::runtime::fault_injection::arm(
            trace,
            std::env::var("METEWAND_ADAPTIVE_CRASH_TARGET")
                .unwrap()
                .parse()
                .unwrap(),
        );
        store.freeze_adaptive_decision(&decision(), 10).unwrap();
    }

    fn run(root: &Path, target: usize) -> (std::process::Output, Vec<String>) {
        let trace = tempfile::NamedTempFile::new().unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "runtime::run_store::adaptive::tests::freeze_child",
                "--nocapture",
            ])
            .env("METEWAND_ADAPTIVE_CRASH_ROOT", root)
            .env("METEWAND_ADAPTIVE_CRASH_TRACE", trace.path())
            .env("METEWAND_ADAPTIVE_CRASH_TARGET", target.to_string())
            .output()
            .unwrap();
        let trace = fs::read_to_string(trace.path())
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect();
        (output, trace)
    }

    #[test]
    fn frozen_budget_survives_process_death_and_shared_replay() {
        for shared in [false, true] {
            let initialize = |root: &Path| {
                if shared {
                    RunStore::open_shared(root).unwrap()
                } else {
                    RunStore::open_buffered(root).unwrap()
                }
            };
            let baseline = tempfile::tempdir().unwrap();
            drop(initialize(baseline.path()));
            let (output, trace) = run(baseline.path(), 0);
            assert!(output.status.success(), "{output:?}");
            for boundary in ["adaptive.freeze.before", "adaptive.freeze.after"] {
                let target = trace.iter().position(|s| s == boundary).unwrap() + 1;
                let root = tempfile::tempdir().unwrap();
                drop(initialize(root.path()));
                assert_eq!(run(root.path(), target).0.status.code(), Some(86));
                let mut store = RunStore::open(root.path()).unwrap();
                let saved = store.adaptive_decision(&decision().group_id).unwrap();
                assert_eq!(saved.is_some(), boundary.ends_with("after"));
                assert_eq!(
                    store.freeze_adaptive_decision(&decision(), 20).unwrap(),
                    decision()
                );
                let mut different = decision();
                different.effective_limit_ns /= 2;
                assert_eq!(
                    store.freeze_adaptive_decision(&different, 30).unwrap(),
                    decision()
                );
            }
        }
    }
}
