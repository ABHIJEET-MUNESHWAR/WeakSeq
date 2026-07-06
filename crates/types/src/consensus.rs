//! Weak-consensus primitives: a stake-weighted validator set, per-batch
//! attestations, and the quorum certificate that finalizes a batch.
//!
//! "Weak" consensus: each batch is confirmed **independently** once a quorum of
//! stake attests to its digest — there is no global total order across batches,
//! which is what lets confirmation happen in a single network round trip.

use alloy_primitives::B256;
use serde::{Deserialize, Serialize};

use crate::batch::BatchId;

/// A validator's identifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ValidatorId(pub u64);

/// A validator's stake weight.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ValidatorStake(pub u128);

/// The active validator set with total stake.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidatorSet {
    validators: Vec<(ValidatorId, ValidatorStake)>,
    total_stake: u128,
}

impl ValidatorSet {
    /// Build a set from `(id, stake)` pairs (ignores zero-stake validators).
    #[must_use]
    pub fn new(validators: Vec<(ValidatorId, ValidatorStake)>) -> Self {
        let validators: Vec<_> = validators.into_iter().filter(|(_, s)| s.0 > 0).collect();
        let total_stake = validators.iter().map(|(_, s)| s.0).sum();
        Self {
            validators,
            total_stake,
        }
    }

    /// Convenience: `n` validators each with equal unit stake.
    #[must_use]
    pub fn uniform(n: u64) -> Self {
        Self::new(
            (0..n)
                .map(|i| (ValidatorId(i), ValidatorStake(1)))
                .collect(),
        )
    }

    #[must_use]
    pub fn total_stake(&self) -> u128 {
        self.total_stake
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.validators.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.validators.is_empty()
    }

    #[must_use]
    pub fn contains(&self, id: ValidatorId) -> bool {
        self.validators.iter().any(|(v, _)| *v == id)
    }

    #[must_use]
    pub fn stake_of(&self, id: ValidatorId) -> Option<ValidatorStake> {
        self.validators
            .iter()
            .find(|(v, _)| *v == id)
            .map(|(_, s)| *s)
    }

    /// Byzantine quorum threshold: strictly more than 2/3 of total stake.
    #[must_use]
    pub fn quorum_threshold(&self) -> u128 {
        self.total_stake * 2 / 3 + 1
    }

    pub fn ids(&self) -> impl Iterator<Item = ValidatorId> + '_ {
        self.validators.iter().map(|(v, _)| *v)
    }
}

/// A validator's signed attestation that batch `batch_id` has digest `digest`.
/// (Signature is modelled as an opaque tag here; real BLS signatures live in the
/// companion FairOrder crypto crate.)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attestation {
    pub validator: ValidatorId,
    pub batch_id: BatchId,
    pub digest: B256,
}

impl Attestation {
    #[must_use]
    pub fn new(validator: ValidatorId, batch_id: BatchId, digest: B256) -> Self {
        Self {
            validator,
            batch_id,
            digest,
        }
    }
}

/// Proof that a quorum of stake attested to a batch digest — the certificate
/// that finalizes the batch under weak consensus.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuorumCertificate {
    pub batch_id: BatchId,
    pub digest: B256,
    pub attestors: Vec<ValidatorId>,
    pub weight: u128,
}

impl QuorumCertificate {
    /// Verify the certificate against a validator set: every attestor must be a
    /// member, and their combined stake must meet the quorum threshold.
    #[must_use]
    pub fn verify(&self, set: &ValidatorSet) -> bool {
        let mut seen = std::collections::BTreeSet::new();
        let mut weight = 0u128;
        for v in &self.attestors {
            if !seen.insert(*v) {
                return false; // duplicate attestor
            }
            match set.stake_of(*v) {
                Some(s) => weight += s.0,
                None => return false, // unknown validator
            }
        }
        weight == self.weight && weight >= set.quorum_threshold()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quorum_threshold_two_thirds() {
        let set = ValidatorSet::uniform(4); // total 4 → threshold 4*2/3+1 = 3
        assert_eq!(set.quorum_threshold(), 3);
        assert_eq!(set.total_stake(), 4);
        assert_eq!(set.len(), 4);
    }

    #[test]
    fn qc_verifies_with_quorum() {
        let set = ValidatorSet::uniform(4);
        let qc = QuorumCertificate {
            batch_id: BatchId(1),
            digest: B256::repeat_byte(9),
            attestors: vec![ValidatorId(0), ValidatorId(1), ValidatorId(2)],
            weight: 3,
        };
        assert!(qc.verify(&set));
    }

    #[test]
    fn qc_rejects_below_quorum() {
        let set = ValidatorSet::uniform(4);
        let qc = QuorumCertificate {
            batch_id: BatchId(1),
            digest: B256::repeat_byte(9),
            attestors: vec![ValidatorId(0), ValidatorId(1)],
            weight: 2,
        };
        assert!(!qc.verify(&set));
    }

    #[test]
    fn qc_rejects_unknown_validator() {
        let set = ValidatorSet::uniform(3);
        let qc = QuorumCertificate {
            batch_id: BatchId(1),
            digest: B256::ZERO,
            attestors: vec![ValidatorId(0), ValidatorId(1), ValidatorId(99)],
            weight: 3,
        };
        assert!(!qc.verify(&set));
    }

    #[test]
    fn qc_rejects_duplicate_attestor() {
        let set = ValidatorSet::uniform(4);
        let qc = QuorumCertificate {
            batch_id: BatchId(1),
            digest: B256::ZERO,
            attestors: vec![ValidatorId(0), ValidatorId(0), ValidatorId(1)],
            weight: 3,
        };
        assert!(!qc.verify(&set));
    }

    #[test]
    fn weighted_stake() {
        let set = ValidatorSet::new(vec![
            (ValidatorId(0), ValidatorStake(10)),
            (ValidatorId(1), ValidatorStake(5)),
            (ValidatorId(2), ValidatorStake(1)),
        ]);
        assert_eq!(set.total_stake(), 16);
        assert_eq!(set.quorum_threshold(), 16 * 2 / 3 + 1); // 11
        assert!(set.contains(ValidatorId(0)));
        assert_eq!(set.stake_of(ValidatorId(1)), Some(ValidatorStake(5)));
    }
}
