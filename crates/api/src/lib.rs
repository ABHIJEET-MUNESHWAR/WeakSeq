//! Event-driven sequencer service (CQRS) + GraphQL adapter.
//!
//! * **Command side** — `submit_order` appends to the mempool.
//! * **Sealing** — `seal_and_clear` drains the mempool into a batch, clears it
//!   at a uniform price, gathers validator attestations, and finalizes the batch
//!   with a quorum certificate.
//! * **Query side** — a read model of confirmed batches (CQRS separation).
#![forbid(unsafe_code)]

mod schema;
mod sequencer;

pub use schema::{build_schema, WeakSeqSchema};
pub use sequencer::{ConfirmedBatch, Sequencer, SequencerConfig};
