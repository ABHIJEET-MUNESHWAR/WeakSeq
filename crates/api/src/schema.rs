//! GraphQL adapter over the [`Sequencer`]. Chosen over REST: the operation
//! surface (submit, seal, status, batch, latest, validators, threshold, depth)
//! exceeds five endpoints.

use std::sync::Arc;

use async_graphql::{Context, EmptySubscription, Enum, Object, Schema, SimpleObject};
use weakseq_types::Side;

use crate::sequencer::{ConfirmedBatch, Sequencer};

pub type WeakSeqSchema = Schema<QueryRoot, MutationRoot, EmptySubscription>;

#[must_use]
pub fn build_schema(sequencer: Arc<Sequencer>) -> WeakSeqSchema {
    Schema::build(QueryRoot, MutationRoot, EmptySubscription)
        .data(sequencer)
        .finish()
}

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
enum OrderSide {
    Buy,
    Sell,
}

impl From<OrderSide> for Side {
    fn from(s: OrderSide) -> Self {
        match s {
            OrderSide::Buy => Side::Buy,
            OrderSide::Sell => Side::Sell,
        }
    }
}

#[derive(SimpleObject)]
struct SequencerStatus {
    healthy: bool,
    mempool_depth: u64,
    confirmed_batches: u64,
    validator_count: u64,
    quorum_threshold: String,
}

#[derive(SimpleObject)]
struct BatchView {
    batch_id: u64,
    clearing_price: Option<u64>,
    matched_quantity: u64,
    fills: u64,
    digest: String,
    certificate_weight: String,
    attestors: u64,
}

impl From<&ConfirmedBatch> for BatchView {
    fn from(c: &ConfirmedBatch) -> Self {
        Self {
            batch_id: c.result.batch_id.0,
            clearing_price: c.result.clearing_price.map(|p| p.ticks()),
            matched_quantity: c.result.matched_quantity.lots(),
            fills: c.result.fills.len() as u64,
            digest: c.result.batch_digest.to_string(),
            certificate_weight: c.certificate.weight.to_string(),
            attestors: c.certificate.attestors.len() as u64,
        }
    }
}

#[derive(Default, Debug)]
pub struct QueryRoot;

#[Object]
impl QueryRoot {
    /// Node status / health.
    async fn status(&self, ctx: &Context<'_>) -> async_graphql::Result<SequencerStatus> {
        let s = ctx.data::<Arc<Sequencer>>()?;
        Ok(SequencerStatus {
            healthy: true,
            mempool_depth: s.mempool_depth() as u64,
            confirmed_batches: s.confirmed_count() as u64,
            validator_count: s.validator_count() as u64,
            quorum_threshold: s.quorum_threshold().to_string(),
        })
    }

    /// Current mempool depth (pending orders).
    async fn mempool_depth(&self, ctx: &Context<'_>) -> async_graphql::Result<u64> {
        Ok(ctx.data::<Arc<Sequencer>>()?.mempool_depth() as u64)
    }

    /// The most recently confirmed batch.
    async fn latest_batch(&self, ctx: &Context<'_>) -> async_graphql::Result<Option<BatchView>> {
        Ok(ctx
            .data::<Arc<Sequencer>>()?
            .latest_batch()
            .as_ref()
            .map(BatchView::from))
    }

    /// A confirmed batch by id.
    async fn batch(&self, ctx: &Context<'_>, id: u64) -> async_graphql::Result<Option<BatchView>> {
        Ok(ctx
            .data::<Arc<Sequencer>>()?
            .batch(id)
            .as_ref()
            .map(BatchView::from))
    }

    /// Number of validators.
    async fn validator_count(&self, ctx: &Context<'_>) -> async_graphql::Result<u64> {
        Ok(ctx.data::<Arc<Sequencer>>()?.validator_count() as u64)
    }
}

#[derive(Default, Debug)]
pub struct MutationRoot;

#[Object]
impl MutationRoot {
    /// Submit a limit order into the mempool. Returns the order id.
    async fn submit_order(
        &self,
        ctx: &Context<'_>,
        side: OrderSide,
        price: u64,
        quantity: u64,
    ) -> async_graphql::Result<u64> {
        let s = ctx.data::<Arc<Sequencer>>()?;
        s.submit_order(side.into(), price, quantity)
            .map(|id| id.0)
            .map_err(|e| async_graphql::Error::new(e.to_string()))
    }

    /// Seal the mempool into a batch and finalize it. Returns the batch view, or
    /// null if the mempool was empty.
    async fn seal_batch(&self, ctx: &Context<'_>) -> async_graphql::Result<Option<BatchView>> {
        let s = ctx.data::<Arc<Sequencer>>()?;
        let confirmed = s
            .seal_and_clear()
            .map_err(|e| async_graphql::Error::new(e.to_string()))?;
        Ok(confirmed.as_ref().map(BatchView::from))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sequencer::SequencerConfig;
    use weakseq_types::ValidatorSet;

    fn schema() -> WeakSeqSchema {
        let seq = Arc::new(Sequencer::new(
            ValidatorSet::uniform(4),
            SequencerConfig::default(),
        ));
        build_schema(seq)
    }

    #[tokio::test]
    async fn status_query() {
        let r = schema()
            .execute("{ status { healthy validatorCount quorumThreshold } }")
            .await;
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert!(r.data.to_string().contains('4'));
    }

    #[tokio::test]
    async fn submit_and_seal_flow() {
        let s = schema();
        let r = s
            .execute("mutation { submitOrder(side: BUY, price: 100, quantity: 5) }")
            .await;
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        let r = s
            .execute("mutation { submitOrder(side: SELL, price: 90, quantity: 5) }")
            .await;
        assert!(r.errors.is_empty(), "{:?}", r.errors);

        let r = s
            .execute("mutation { sealBatch { batchId matchedQuantity attestors } }")
            .await;
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        let body = r.data.to_string();
        assert!(body.contains("matchedQuantity"));

        let r = s
            .execute("{ latestBatch { batchId matchedQuantity certificateWeight } }")
            .await;
        assert!(r.errors.is_empty(), "{:?}", r.errors);
    }

    #[tokio::test]
    async fn seal_empty_returns_null() {
        let r = schema().execute("mutation { sealBatch { batchId } }").await;
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert!(r.data.to_string().contains("null"));
    }
}
