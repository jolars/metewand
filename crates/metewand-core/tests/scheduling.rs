use std::collections::BTreeSet;

use metewand_core::{
    SCHEDULING_POLICY_VERSION,
    manifest::RunOrder,
    records::{ContentDigest, DerivedSeedRecord, RecordId},
    scheduling::{
        ComparisonBlock, SchedulingSlot, derive_block_scheduling_priority, schedule_slots,
    },
};
use serde_json::Value;

fn seed() -> DerivedSeedRecord {
    DerivedSeedRecord {
        value: 0,
        derivation_digest: ContentDigest::new([17; 32]),
    }
}

fn slots() -> Vec<SchedulingSlot> {
    let mut slots = Vec::new();
    for dataset in 1..=2 {
        for problem in 3..=4 {
            for implementation in 0..3 {
                for implementation_repetition in 0..2 {
                    for measurement_index in 0..2 {
                        slots.push(SchedulingSlot {
                            observation_round: 0,
                            logical_slot: RecordId::from_digest([slots.len() as u8; 32]),
                            comparison_block: ComparisonBlock {
                                dataset_instance: RecordId::from_digest([dataset; 32]),
                                problem_configuration: RecordId::from_digest([problem; 32]),
                                implementation_repetition,
                                measurement_index,
                            },
                            scheduling_priority: ContentDigest::new([2 - implementation; 32]),
                        });
                    }
                }
            }
        }
    }
    slots
}

#[test]
fn sequential_preserves_input_order_including_empty_and_singleton_schedules() {
    let slots = slots();
    for slots in [&slots[..], &slots[..1], &slots[..0]] {
        assert_eq!(
            schedule_slots(RunOrder::Sequential, &seed(), slots),
            (0..slots.len()).collect::<Vec<_>>()
        );
    }
    assert!(schedule_slots(RunOrder::Randomized, &seed(), &[]).is_empty());
    assert_eq!(
        schedule_slots(RunOrder::Randomized, &seed(), &slots[..1]),
        [0]
    );
}

#[test]
fn randomized_schedules_complete_comparison_blocks_and_their_members_by_priority() {
    let slots = slots();
    let order = schedule_slots(RunOrder::Randomized, &seed(), &slots);
    assert_eq!(order.len(), 48);
    assert_eq!(
        order.iter().copied().collect::<BTreeSet<_>>(),
        (0..48).collect()
    );
    let mut previous_block = None;
    let mut seen = BTreeSet::new();
    for members in order.as_chunks::<3>().0 {
        let block = slots[members[0]].comparison_block;
        assert!(seen.insert(block));
        assert!(
            members
                .iter()
                .all(|&index| slots[index].comparison_block == block)
        );
        let priority = derive_block_scheduling_priority(&seed(), &block);
        if let Some(previous) = previous_block {
            assert!(previous < priority);
        }
        previous_block = Some(priority);
        assert_eq!(
            members
                .iter()
                .map(|&index| slots[index].scheduling_priority)
                .collect::<Vec<_>>(),
            [
                ContentDigest::new([0; 32]),
                ContentDigest::new([1; 32]),
                ContentDigest::new([2; 32]),
            ]
        );
    }
    assert_eq!(seen.len(), 16);
}

#[test]
fn randomized_order_depends_on_identities_and_full_seed_digest() {
    let slots = slots();
    let ordered_ids = |slots: &[SchedulingSlot], seed: &DerivedSeedRecord| {
        schedule_slots(RunOrder::Randomized, seed, slots)
            .into_iter()
            .map(|index| slots[index].logical_slot)
            .collect::<Vec<_>>()
    };
    let expected = ordered_ids(&slots, &seed());
    let mut reversed = slots.clone();
    reversed.reverse();
    assert_eq!(ordered_ids(&reversed, &seed()), expected);

    let mut extended = slots.clone();
    let mut extra = slots[0];
    extra.logical_slot = RecordId::from_digest([250; 32]);
    extended.insert(0, extra);
    extra.logical_slot = RecordId::from_digest([251; 32]);
    extra.comparison_block.dataset_instance = RecordId::from_digest([250; 32]);
    extended.push(extra);
    assert_eq!(
        ordered_ids(&extended, &seed())
            .into_iter()
            .filter(|id| id.digest()[0] < 250)
            .collect::<Vec<_>>(),
        expected
    );
    let mut changed_seed = seed();
    changed_seed.derivation_digest = ContentDigest::new([18; 32]);
    assert_ne!(ordered_ids(&slots, &changed_seed), expected);
}

#[test]
fn equal_slot_priorities_use_typed_slot_identities_to_break_ties() {
    let mut slots = slots()[..3].to_vec();
    let block = slots[0].comparison_block;
    for slot in &mut slots {
        slot.comparison_block = block;
        slot.scheduling_priority = ContentDigest::new([0; 32]);
    }
    slots.reverse();
    assert_eq!(
        schedule_slots(RunOrder::Randomized, &seed(), &slots),
        [2, 1, 0]
    );
}

#[test]
fn block_priorities_match_version_one_conformance_vectors() {
    let fixture: Value =
        serde_json::from_str(include_str!("../../../fixtures/scheduling/v1.json")).unwrap();
    assert_eq!(
        fixture["scheduling_policy_version"],
        SCHEDULING_POLICY_VERSION
    );
    for vector in fixture["blocks"].as_array().unwrap() {
        let block = ComparisonBlock {
            dataset_instance: vector["dataset_instance"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap(),
            problem_configuration: vector["problem_configuration"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap(),
            implementation_repetition: vector["implementation_repetition"].as_u64().unwrap(),
            measurement_index: vector["measurement_index"].as_u64().unwrap(),
        };
        let digest = derive_block_scheduling_priority(&seed(), &block);
        let hex = digest
            .bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        assert_eq!(hex, vector["priority"].as_str().unwrap());
    }
}

#[test]
fn fresh_rounds_interleave_unequal_schedules_within_comparison_blocks() {
    let mut slots = slots()[..1].to_vec();
    let base = slots[0];
    slots.clear();
    for implementation in 0..2u8 {
        for round in 0..(3 - u64::from(implementation)) {
            slots.push(SchedulingSlot {
                observation_round: round,
                logical_slot: RecordId::from_digest([implementation * 3 + round as u8; 32]),
                scheduling_priority: ContentDigest::new([1 - implementation; 32]),
                ..base
            });
        }
    }
    slots.reverse();
    let order = schedule_slots(RunOrder::Randomized, &seed(), &slots);
    assert_eq!(
        order
            .iter()
            .map(|&i| slots[i].observation_round)
            .collect::<Vec<_>>(),
        [0, 0, 1, 1, 2]
    );
    assert_eq!(
        order
            .iter()
            .map(|&i| slots[i].scheduling_priority.bytes()[0])
            .collect::<Vec<_>>(),
        [0, 1, 0, 1, 1]
    );
}
