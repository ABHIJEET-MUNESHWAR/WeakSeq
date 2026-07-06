//! Errors for the sequencer domain.

use thiserror::Error;

pub type SeqResult<T> = Result<T, SeqError>;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SeqError {
    #[error("order price must be greater than zero")]
    ZeroPrice,

    #[error("order quantity must be greater than zero")]
    ZeroQuantity,

    #[error("batch {0} is empty; nothing to clear")]
    EmptyBatch(u64),

    #[error("batch {0} already sealed")]
    BatchSealed(u64),

    #[error("unknown validator: {0}")]
    UnknownValidator(u64),

    #[error("validator {validator} equivocated on batch {batch}: two distinct digests")]
    Equivocation { validator: u64, batch: u64 },

    #[error("duplicate attestation from validator {validator} for batch {batch}")]
    DuplicateAttestation { validator: u64, batch: u64 },

    #[error("digest mismatch for batch {0}: attestation does not match proposed batch")]
    DigestMismatch(u64),

    #[error("quorum not reached for batch {batch}: have {have}, need {need}")]
    QuorumNotReached { batch: u64, have: u128, need: u128 },

    #[error("rate limit exceeded")]
    RateLimited,

    #[error("sequencer channel closed")]
    ChannelClosed,
}

impl SeqError {
    #[must_use]
    pub const fn is_retryable(&self) -> bool {
        matches!(self, Self::RateLimited)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retryable_flag() {
        assert!(SeqError::RateLimited.is_retryable());
        assert!(!SeqError::ZeroPrice.is_retryable());
    }

    #[test]
    fn renders_message() {
        assert!(SeqError::EmptyBatch(3).to_string().contains("empty"));
    }
}
