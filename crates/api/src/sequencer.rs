//! The sequencer: a block-less, event-driven batch engine with CQRS read model.

use std::num::NonZeroU32;
use std::sync::atomic::{AtomicU64, Ordering};

use governor::{clock::DefaultClock, state::InMemoryState, state::NotKeyed, Quota, RateLimiter};
use parking_lot::Mutex;
use tracing::{info, instrument};
use weakseq_auction::{AuctionEngine, UniformPriceAuction};
use weakseq_consensus::{AttestOutcome, ConsensusEngine};
use weakseq_types::{
    Attestation, Batch, BatchId, ClearingResult, Order, OrderId, QuorumCertificate, SeqError,
    SeqResult, Side, ValidatorSet,
};

/// A batch that has been cleared **and** finalized by a quorum certificate.
#[derive(Clone, Debug)]
pub struct ConfirmedBatch {
    pub result: ClearingResult,
    pub certificate: QuorumCertificate,
}

/// Sequencer configuration.
#[derive(Clone, Copy, Debug)]
pub struct SequencerConfig {
    /// Max order submissions per second (rate limit / backpressure).
    pub max_orders_per_sec: u32,
    /// Number of honest validators that attest each batch in this node's
    /// self-contained demo (must be ≤ validator-set size).
    pub honest_validators: u64,
}

impl Default for SequencerConfig {
    fn default() -> Self {
        Self {
            max_orders_per_sec: 50_000,
            honest_validators: u64::MAX,
        }
    }
}

type DirectLimiter = RateLimiter<NotKeyed, InMemoryState, DefaultClock>;

/// The core sequencer. Thread-safe; cloneable handles share state via `Arc`.
pub struct Sequencer {
    mempool: Mutex<Vec<Order>>,
    confirmed: Mutex<Vec<ConfirmedBatch>>,
    consensus: ConsensusEngine,
    auction: UniformPriceAuction,
    limiter: DirectLimiter,
    next_order_id: AtomicU64,
    next_batch_id: AtomicU64,
    honest: u64,
}

impl std::fmt::Debug for Sequencer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sequencer")
            .field("mempool_depth", &self.mempool.lock().len())
            .field("confirmed", &self.confirmed.lock().len())
            .field("validators", &self.consensus.validators().len())
            .finish()
    }
}

impl Sequencer {
    #[must_use]
    pub fn new(validators: ValidatorSet, config: SequencerConfig) -> Self {
        let per_sec = NonZeroU32::new(config.max_orders_per_sec.max(1)).unwrap();
        let honest = config.honest_validators.min(validators.len() as u64);
        Self {
            mempool: Mutex::new(Vec::new()),
            confirmed: Mutex::new(Vec::new()),
            consensus: ConsensusEngine::new(validators),
            auction: UniformPriceAuction,
            limiter: RateLimiter::direct(Quota::per_second(per_sec)),
            next_order_id: AtomicU64::new(0),
            next_batch_id: AtomicU64::new(0),
            honest,
        }
    }

    // ---- Command side ----

    /// Submit an order to the mempool. Returns the assigned id.
    pub fn submit_order(&self, side: Side, price: u64, quantity: u64) -> SeqResult<OrderId> {
        if self.limiter.check().is_err() {
            metrics::counter!("weakseq_orders_rate_limited_total").increment(1);
            return Err(SeqError::RateLimited);
        }
        metrics::counter!("weakseq_orders_submitted_total").increment(1);
        let id = self.next_order_id.fetch_add(1, Ordering::Relaxed);
        let order = Order::new(id, side, price, quantity)?;
        self.mempool.lock().push(order);
        Ok(OrderId(id))
    }

    /// Seal the current mempool into a batch, clear it, and finalize it under
    /// weak consensus. Returns `None` if the mempool was empty.
    #[instrument(skip(self))]
    pub fn seal_and_clear(&self) -> SeqResult<Option<ConfirmedBatch>> {
        let orders = {
            let mut mp = self.mempool.lock();
            if mp.is_empty() {
                return Ok(None);
            }
            std::mem::take(&mut *mp)
        };

        let batch_id = self.next_batch_id.fetch_add(1, Ordering::Relaxed);
        let batch = Batch::new(batch_id, orders);
        let digest = batch.digest();
        metrics::histogram!("weakseq_batch_orders").record(batch.len() as f64);
        let result = self.auction.clear(&batch);

        // Gather attestations from the honest validator subset until a quorum
        // certificate forms (models a single-round-trip confirmation).
        let mut certificate = None;
        for v in self.consensus.validators().ids().take(self.honest as usize) {
            let outcome = self
                .consensus
                .accept(Attestation::new(v, BatchId(batch_id), digest))?;
            if let AttestOutcome::Finalized(qc) = outcome {
                certificate = Some(qc);
                break;
            }
        }

        let certificate = certificate.ok_or(SeqError::QuorumNotReached {
            batch: batch_id,
            have: 0,
            need: self.consensus.validators().quorum_threshold(),
        })?;

        let confirmed = ConfirmedBatch {
            result,
            certificate,
        };
        self.confirmed.lock().push(confirmed.clone());
        metrics::counter!("weakseq_batches_sealed_total").increment(1);
        metrics::histogram!("weakseq_batch_matched_quantity")
            .record(confirmed.result.matched_quantity.lots() as f64);
        info!(
            batch = batch_id,
            matched = confirmed.result.matched_quantity.lots(),
            "batch confirmed"
        );
        Ok(Some(confirmed))
    }

    // ---- Query side (read model) ----

    #[must_use]
    pub fn mempool_depth(&self) -> usize {
        self.mempool.lock().len()
    }

    #[must_use]
    pub fn confirmed_count(&self) -> usize {
        self.confirmed.lock().len()
    }

    #[must_use]
    pub fn latest_batch(&self) -> Option<ConfirmedBatch> {
        self.confirmed.lock().last().cloned()
    }

    #[must_use]
    pub fn batch(&self, id: u64) -> Option<ConfirmedBatch> {
        self.confirmed
            .lock()
            .iter()
            .find(|b| b.result.batch_id == BatchId(id))
            .cloned()
    }

    #[must_use]
    pub fn validator_count(&self) -> usize {
        self.consensus.validators().len()
    }

    #[must_use]
    pub fn quorum_threshold(&self) -> u128 {
        self.consensus.validators().quorum_threshold()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seq() -> Sequencer {
        Sequencer::new(ValidatorSet::uniform(4), SequencerConfig::default())
    }

    #[test]
    fn submit_and_seal_crossing_batch() {
        let s = seq();
        s.submit_order(Side::Buy, 100, 5).unwrap();
        s.submit_order(Side::Sell, 90, 5).unwrap();
        assert_eq!(s.mempool_depth(), 2);

        let confirmed = s.seal_and_clear().unwrap().expect("batch");
        assert_eq!(confirmed.result.matched_quantity.lots(), 5);
        assert!(confirmed.certificate.verify(s.consensus.validators()));
        assert_eq!(s.mempool_depth(), 0);
        assert_eq!(s.confirmed_count(), 1);
    }

    #[test]
    fn seal_empty_is_none() {
        let s = seq();
        assert!(s.seal_and_clear().unwrap().is_none());
    }

    #[test]
    fn no_cross_still_finalizes_no_trade() {
        let s = seq();
        s.submit_order(Side::Buy, 90, 5).unwrap();
        s.submit_order(Side::Sell, 100, 5).unwrap();
        let c = s.seal_and_clear().unwrap().unwrap();
        assert!(c.result.is_no_trade());
        // Even a no-trade batch is confirmed by consensus (state advances).
        assert!(c.certificate.verify(s.consensus.validators()));
    }

    #[test]
    fn rate_limit_enforced() {
        let s = Sequencer::new(
            ValidatorSet::uniform(4),
            SequencerConfig {
                max_orders_per_sec: 1,
                honest_validators: u64::MAX,
            },
        );
        s.submit_order(Side::Buy, 100, 1).unwrap();
        assert_eq!(
            s.submit_order(Side::Buy, 100, 1),
            Err(SeqError::RateLimited)
        );
    }

    #[test]
    fn rejects_zero_price() {
        let s = seq();
        assert_eq!(s.submit_order(Side::Buy, 0, 5), Err(SeqError::ZeroPrice));
    }

    #[test]
    fn query_batch_by_id() {
        let s = seq();
        s.submit_order(Side::Buy, 100, 5).unwrap();
        s.submit_order(Side::Sell, 95, 5).unwrap();
        s.seal_and_clear().unwrap();
        assert!(s.batch(0).is_some());
        assert!(s.batch(999).is_none());
        assert_eq!(s.validator_count(), 4);
        assert_eq!(s.quorum_threshold(), 3);
    }

    #[test]
    fn multiple_batches_accumulate() {
        let s = seq();
        for round in 0..3 {
            s.submit_order(Side::Buy, 100 + round, 2).unwrap();
            s.submit_order(Side::Sell, 90, 2).unwrap();
            s.seal_and_clear().unwrap();
        }
        assert_eq!(s.confirmed_count(), 3);
        assert_eq!(s.latest_batch().unwrap().result.batch_id, BatchId(2));
    }
}
