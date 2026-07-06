//! Batch and clearing-result model. A [`Batch`] is a set of orders sealed for a
//! single auction round; [`ClearingResult`] is its deterministic outcome.

use alloy_primitives::{keccak256, B256};
use serde::{Deserialize, Serialize};

use crate::order::{Order, OrderId, Price, Quantity, Side};

/// Monotonic auction-round identifier (there are no blocks — just ordered rounds).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BatchId(pub u64);

impl std::fmt::Display for BatchId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "batch#{}", self.0)
    }
}

/// A sealed set of orders to be cleared in one auction round.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Batch {
    pub id: u64,
    pub orders: Vec<Order>,
}

impl Batch {
    #[must_use]
    pub fn new(id: u64, orders: Vec<Order>) -> Self {
        Self { id, orders }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.orders.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.orders.len()
    }

    /// Content digest binding the *set* of orders to the batch id. Because we
    /// sort by order id first, the digest is **independent of arrival order** —
    /// the MEV-free property: reordering within a batch cannot change the digest.
    #[must_use]
    pub fn digest(&self) -> B256 {
        let mut ids: Vec<&Order> = self.orders.iter().collect();
        ids.sort_unstable_by_key(|o| o.id.0);
        let mut buf = Vec::with_capacity(8 + self.orders.len() * 25);
        buf.extend_from_slice(&self.id.to_be_bytes());
        for o in ids {
            buf.extend_from_slice(&o.id.0.to_be_bytes());
            buf.push(match o.side {
                Side::Buy => 0,
                Side::Sell => 1,
            });
            buf.extend_from_slice(&o.price.ticks().to_be_bytes());
            buf.extend_from_slice(&o.quantity.lots().to_be_bytes());
        }
        keccak256(&buf)
    }
}

/// A single matched trade produced by clearing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fill {
    pub order_id: OrderId,
    pub side: Side,
    pub quantity: Quantity,
}

/// Deterministic result of clearing a batch at a single uniform price.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClearingResult {
    pub batch_id: BatchId,
    /// The uniform clearing price, or `None` if nothing crossed.
    pub clearing_price: Option<Price>,
    /// Total matched quantity (buy side == sell side).
    pub matched_quantity: Quantity,
    pub fills: Vec<Fill>,
    /// Digest of the sealed batch (what validators attest to).
    pub batch_digest: B256,
}

impl ClearingResult {
    #[must_use]
    pub fn is_no_trade(&self) -> bool {
        self.clearing_price.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn order(id: u64, side: Side, price: u64, qty: u64) -> Order {
        Order::new(id, side, price, qty).unwrap()
    }

    #[test]
    fn digest_is_order_independent() {
        let a = Batch::new(
            1,
            vec![order(1, Side::Buy, 100, 5), order(2, Side::Sell, 90, 5)],
        );
        let b = Batch::new(
            1,
            vec![order(2, Side::Sell, 90, 5), order(1, Side::Buy, 100, 5)],
        );
        assert_eq!(a.digest(), b.digest());
    }

    #[test]
    fn digest_changes_with_content() {
        let a = Batch::new(1, vec![order(1, Side::Buy, 100, 5)]);
        let b = Batch::new(1, vec![order(1, Side::Buy, 101, 5)]);
        assert_ne!(a.digest(), b.digest());
    }

    #[test]
    fn empty_batch_helpers() {
        let e = Batch::new(7, vec![]);
        assert!(e.is_empty());
        assert_eq!(e.len(), 0);
    }
}
