//! Pure deterministic ordering over resolved one-shot measurement slots.

use std::collections::BTreeMap;

use crate::core::{
    manifest::RunOrder,
    records::{
        ContentDigest, DatasetInstanceRecord, LogicalAttemptSlotRecord, ProblemConfigurationRecord,
        RecordId,
    },
};

#[cfg(test)]
use crate::core::{planning::SCHEDULING_PRIORITY_DOMAIN, records::DerivedSeedRecord};
#[cfg(test)]
use sha2::{Digest, Sha256};

/// Comparable measurements that differ only in implementation configuration.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ComparisonBlock {
    /// Materialized input shared by the implementations.
    pub dataset_instance: RecordId<DatasetInstanceRecord>,
    /// Configured computation shared by the implementations.
    pub problem_configuration: RecordId<ProblemConfigurationRecord>,
    /// Zero-based implementation replication.
    pub implementation_repetition: u64,
    /// Zero-based measurement repetition.
    pub measurement_index: u64,
}

/// Scheduling inputs for one measured slot, independent of executor state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchedulingSlot {
    /// Fresh-sequence round; zero for existing delivery modes.
    pub observation_round: u64,
    /// Stable identity used to break equal within-block priorities.
    pub logical_slot: RecordId<LogicalAttemptSlotRecord>,
    /// Comparison group resolved after dataset materialization.
    pub comparison_block: ComparisonBlock,
    /// Within-block priority attached during logical planning.
    pub scheduling_priority: ContentDigest,
}

/// Derives a version-1 priority from the seed digest and typed block fields.
///
/// Typed identities use their complete UTF-8 spelling, separated by NUL.
/// Indices use eight-byte big-endian encoding, also separated by NUL.
#[must_use]
#[cfg(test)]
pub fn derive_block_scheduling_priority(
    scheduling_seed: &DerivedSeedRecord,
    block: &ComparisonBlock,
) -> ContentDigest {
    let mut hasher = Sha256::new();
    hasher.update(SCHEDULING_PRIORITY_DOMAIN.as_bytes());
    hasher.update([0]);
    hasher.update(scheduling_seed.derivation_digest.bytes());
    hasher.update(b"\0block\0");
    hasher.update(block.dataset_instance.to_string().as_bytes());
    hasher.update([0]);
    hasher.update(block.problem_configuration.to_string().as_bytes());
    hasher.update([0]);
    hasher.update(block.implementation_repetition.to_be_bytes());
    hasher.update([0]);
    hasher.update(block.measurement_index.to_be_bytes());
    ContentDigest::new(hasher.finalize().into())
}

/// Returns input indices in deterministic dispatch order for one experiment.
///
/// Sequential order retains the caller's deterministic expansion order.
/// Randomized order preserves first-seen dataset and problem configurations,
/// visits repetition indices in order, and sorts implementations by their seeded
/// slot priorities. Identity tie-breakers resolve equal priorities independently
/// of member order. No priority depends on the selected member set.
/// Warm-ups belong to the worker lifecycle and run before their measurement.
#[must_use]
pub fn schedule_slots(run_order: RunOrder, slots: &[SchedulingSlot]) -> Vec<usize> {
    let mut order = (0..slots.len()).collect::<Vec<_>>();
    if run_order == RunOrder::Randomized {
        let mut datasets = BTreeMap::new();
        let mut problems = BTreeMap::new();
        for slot in slots {
            let block = slot.comparison_block;
            let next_dataset = datasets.len();
            datasets
                .entry(block.dataset_instance)
                .or_insert(next_dataset);
            let next_problem = problems.len();
            problems
                .entry((block.dataset_instance, block.problem_configuration))
                .or_insert(next_problem);
        }
        order.sort_by_cached_key(|&index| {
            let slot = &slots[index];
            let block = slot.comparison_block;
            (
                datasets[&block.dataset_instance],
                problems[&(block.dataset_instance, block.problem_configuration)],
                block.implementation_repetition,
                block.measurement_index,
                slot.observation_round,
                slot.scheduling_priority,
                slot.logical_slot,
            )
        });
    }
    order
}
