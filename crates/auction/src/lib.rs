//! Uniform-price batch auction.
//!
//! All orders in a batch clear at **one price**. This is the MEV-free core of a
//! Pod-style market: since every trade in the round executes at the same price,
//! there is no advantage to being early — reordering the batch cannot change any
//! participant's outcome. Clearing is deterministic and integer-only (no floats).
#![forbid(unsafe_code)]

mod uniform;

pub use uniform::UniformPriceAuction;

use weakseq_types::{Batch, ClearingResult};

/// Strategy interface for clearing a batch (Strategy pattern). Alternative
/// mechanisms (e.g. frequent-batch call auction, pro-rata) can implement this.
pub trait AuctionEngine: Send + Sync {
    /// Clear a sealed batch into a deterministic result.
    fn clear(&self, batch: &Batch) -> ClearingResult;
}
