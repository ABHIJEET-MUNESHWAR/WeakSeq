//! Domain types for **WeakSeq** — a block-less, MEV-free batch-auction sequencer
//! confirmed by **weak consensus**.
//!
//! Pod-style design: transactions stream in continuously and are cleared in
//! *batches* (auction rounds). Because every order in a batch clears at one
//! uniform price, being early in the batch confers no advantage — there is no
//! MEV and no benefit to transaction reordering. Each batch is confirmed
//! independently by a validator quorum in a single round trip ("weak"
//! consensus: no global total order across batches).
#![forbid(unsafe_code)]

mod batch;
mod consensus;
mod error;
mod order;

pub use batch::{Batch, BatchId, ClearingResult, Fill};
pub use consensus::{Attestation, QuorumCertificate, ValidatorId, ValidatorSet, ValidatorStake};
pub use error::{SeqError, SeqResult};
pub use order::{Order, OrderId, Price, Quantity, Side};

pub use alloy_primitives::B256;
