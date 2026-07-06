//! Weak-consensus engine: collect per-batch attestations and, once a quorum of
//! stake agrees on a single digest, emit a [`QuorumCertificate`] that finalizes
//! the batch. Each batch is confirmed **independently** (no cross-batch total
//! order), enabling single-round-trip confirmation.
//!
//! Safety properties enforced:
//! * **Membership** — only known validators may attest.
//! * **No equivocation** — a validator cannot attest two different digests for
//!   the same batch.
//! * **Idempotence** — duplicate attestations are rejected, not double-counted.
#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use parking_lot::Mutex;
use tracing::{debug, instrument};
use weakseq_types::{
    Attestation, BatchId, QuorumCertificate, SeqError, SeqResult, ValidatorId, ValidatorSet, B256,
};

/// Outcome of accepting a single attestation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttestOutcome {
    /// Accepted; quorum not yet reached.
    Accumulated {
        batch: BatchId,
        weight: u128,
        needed: u128,
    },
    /// Accepted; this attestation completed the quorum.
    Finalized(QuorumCertificate),
    /// Already finalized; the existing certificate is returned.
    AlreadyFinalized(QuorumCertificate),
}

#[derive(Debug, Default)]
struct BatchTally {
    /// digest → set of (validator → stake) that attested it.
    votes: BTreeMap<B256, BTreeMap<ValidatorId, u128>>,
    /// validator → digest it has already attested (equivocation guard).
    voted: BTreeMap<ValidatorId, B256>,
    certificate: Option<QuorumCertificate>,
}

/// Thread-safe weak-consensus engine over a fixed validator set.
#[derive(Debug)]
pub struct ConsensusEngine {
    validators: ValidatorSet,
    tallies: Mutex<BTreeMap<u64, BatchTally>>,
}

impl ConsensusEngine {
    #[must_use]
    pub fn new(validators: ValidatorSet) -> Self {
        Self {
            validators,
            tallies: Mutex::new(BTreeMap::new()),
        }
    }

    #[must_use]
    pub fn validators(&self) -> &ValidatorSet {
        &self.validators
    }

    /// Fetch a finalized certificate for a batch, if one exists.
    #[must_use]
    pub fn certificate(&self, batch: BatchId) -> Option<QuorumCertificate> {
        self.tallies
            .lock()
            .get(&batch.0)
            .and_then(|t| t.certificate.clone())
    }

    /// Accept an attestation, forming a quorum certificate when the threshold is
    /// crossed. Enforces membership, anti-equivocation and idempotence.
    #[instrument(skip(self), fields(batch = att.batch_id.0, validator = att.validator.0))]
    pub fn accept(&self, att: Attestation) -> SeqResult<AttestOutcome> {
        // Membership check.
        let stake = self
            .validators
            .stake_of(att.validator)
            .ok_or(SeqError::UnknownValidator(att.validator.0))?
            .0;

        let mut tallies = self.tallies.lock();
        let tally = tallies.entry(att.batch_id.0).or_default();

        if let Some(cert) = &tally.certificate {
            return Ok(AttestOutcome::AlreadyFinalized(cert.clone()));
        }

        // Anti-equivocation & idempotence.
        match tally.voted.get(&att.validator) {
            Some(prev) if *prev == att.digest => {
                return Err(SeqError::DuplicateAttestation {
                    validator: att.validator.0,
                    batch: att.batch_id.0,
                });
            }
            Some(_) => {
                return Err(SeqError::Equivocation {
                    validator: att.validator.0,
                    batch: att.batch_id.0,
                });
            }
            None => {}
        }

        tally.voted.insert(att.validator, att.digest);
        tally
            .votes
            .entry(att.digest)
            .or_default()
            .insert(att.validator, stake);

        let threshold = self.validators.quorum_threshold();
        let group = &tally.votes[&att.digest];
        let weight: u128 = group.values().sum();

        if weight >= threshold {
            let mut attestors: Vec<ValidatorId> = group.keys().copied().collect();
            attestors.sort_unstable();
            let cert = QuorumCertificate {
                batch_id: att.batch_id,
                digest: att.digest,
                attestors,
                weight,
            };
            tally.certificate = Some(cert.clone());
            debug!(weight, threshold, "batch finalized under weak consensus");
            Ok(AttestOutcome::Finalized(cert))
        } else {
            Ok(AttestOutcome::Accumulated {
                batch: att.batch_id,
                weight,
                needed: threshold,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(n: u8) -> B256 {
        B256::repeat_byte(n)
    }

    fn engine(n: u64) -> ConsensusEngine {
        ConsensusEngine::new(ValidatorSet::uniform(n))
    }

    fn att(v: u64, batch: u64, d: B256) -> Attestation {
        Attestation::new(ValidatorId(v), BatchId(batch), d)
    }

    #[test]
    fn finalizes_at_quorum() {
        let e = engine(4); // threshold 3
        assert!(matches!(
            e.accept(att(0, 1, digest(1))).unwrap(),
            AttestOutcome::Accumulated { .. }
        ));
        assert!(matches!(
            e.accept(att(1, 1, digest(1))).unwrap(),
            AttestOutcome::Accumulated { .. }
        ));
        let out = e.accept(att(2, 1, digest(1))).unwrap();
        match out {
            AttestOutcome::Finalized(qc) => {
                assert_eq!(qc.weight, 3);
                assert!(qc.verify(e.validators()));
            }
            other => panic!("expected finalized, got {other:?}"),
        }
    }

    #[test]
    fn rejects_unknown_validator() {
        let e = engine(3);
        assert_eq!(
            e.accept(att(99, 1, digest(1))),
            Err(SeqError::UnknownValidator(99))
        );
    }

    #[test]
    fn rejects_equivocation() {
        let e = engine(4);
        e.accept(att(0, 1, digest(1))).unwrap();
        assert_eq!(
            e.accept(att(0, 1, digest(2))),
            Err(SeqError::Equivocation {
                validator: 0,
                batch: 1
            })
        );
    }

    #[test]
    fn rejects_duplicate() {
        let e = engine(4);
        e.accept(att(0, 1, digest(1))).unwrap();
        assert_eq!(
            e.accept(att(0, 1, digest(1))),
            Err(SeqError::DuplicateAttestation {
                validator: 0,
                batch: 1
            })
        );
    }

    #[test]
    fn after_finalize_returns_existing_cert() {
        let e = engine(3); // threshold 3 → need all
        e.accept(att(0, 1, digest(1))).unwrap();
        e.accept(att(1, 1, digest(1))).unwrap();
        let _ = e.accept(att(2, 1, digest(1))).unwrap();
        // A late attestation on the finalized batch returns the existing cert.
        let out = e.accept(att(2, 1, digest(1))).unwrap();
        assert!(matches!(out, AttestOutcome::AlreadyFinalized(_)));
        assert!(e.certificate(BatchId(1)).is_some());
    }

    #[test]
    fn split_vote_never_finalizes() {
        let e = engine(4); // threshold 3
                           // 2 for digest(1), 2 for digest(2): neither reaches 3.
        e.accept(att(0, 5, digest(1))).unwrap();
        e.accept(att(1, 5, digest(1))).unwrap();
        e.accept(att(2, 5, digest(2))).unwrap();
        let out = e.accept(att(3, 5, digest(2))).unwrap();
        assert!(matches!(out, AttestOutcome::Accumulated { .. }));
        assert!(e.certificate(BatchId(5)).is_none());
    }
}
