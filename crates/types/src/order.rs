//! Order model for the batch auction. Prices and quantities are integer
//! newtypes (fixed-point) so there is no floating-point non-determinism.

use serde::{Deserialize, Serialize};

use crate::error::SeqError;

/// Buy or sell.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Side {
    Buy,
    Sell,
}

/// A limit price in integer ticks (e.g. micro-USD). Deterministic, no floats.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Price(u64);

impl Price {
    #[inline]
    pub fn new(ticks: u64) -> Result<Self, SeqError> {
        if ticks == 0 {
            return Err(SeqError::ZeroPrice);
        }
        Ok(Self(ticks))
    }

    #[inline]
    #[must_use]
    pub const fn ticks(self) -> u64 {
        self.0
    }
}

/// An order quantity in integer lots.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Quantity(u64);

impl Quantity {
    /// The additive identity — a valid "nothing matched" quantity.
    pub const ZERO: Self = Self(0);

    #[inline]
    pub fn new(lots: u64) -> Result<Self, SeqError> {
        if lots == 0 {
            return Err(SeqError::ZeroQuantity);
        }
        Ok(Self(lots))
    }

    #[inline]
    #[must_use]
    pub const fn lots(self) -> u64 {
        self.0
    }

    #[inline]
    #[must_use]
    pub fn min(self, other: Self) -> Self {
        Self(self.0.min(other.0))
    }

    #[inline]
    #[must_use]
    pub fn saturating_sub(self, other: Self) -> Self {
        Self(self.0.saturating_sub(other.0))
    }
}

/// A monotonically-assigned order identifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OrderId(pub u64);

/// A limit order submitted to the sequencer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Order {
    pub id: OrderId,
    pub side: Side,
    pub price: Price,
    pub quantity: Quantity,
}

impl Order {
    pub fn new(id: u64, side: Side, price: u64, quantity: u64) -> Result<Self, SeqError> {
        Ok(Self {
            id: OrderId(id),
            side,
            price: Price::new(price)?,
            quantity: Quantity::new(quantity)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_zero_price_and_qty() {
        assert_eq!(Price::new(0), Err(SeqError::ZeroPrice));
        assert_eq!(Quantity::new(0), Err(SeqError::ZeroQuantity));
    }

    #[test]
    fn quantity_min_and_sub() {
        let a = Quantity::new(10).unwrap();
        let b = Quantity::new(4).unwrap();
        assert_eq!(a.min(b), b);
        assert_eq!(a.saturating_sub(b).lots(), 6);
        assert_eq!(b.saturating_sub(a).lots(), 0);
    }

    #[test]
    fn order_construction() {
        let o = Order::new(1, Side::Buy, 100, 5).unwrap();
        assert_eq!(o.price.ticks(), 100);
        assert_eq!(o.quantity.lots(), 5);
    }
}
