//! The uniform clearing-price algorithm.
//!
//! For each candidate price `p`:
//!
//!   * demand(p) = Σ quantity of buys with limit ≥ p
//!   * supply(p) = Σ quantity of sells with limit ≤ p
//!   * matched(p) = min(demand, supply)
//!
//! The clearing price maximizes `matched(p)`; ties are broken to minimize the
//! demand/supply imbalance, then by the lower price (fully deterministic).
//!
//! Complexity: `O(k log k)` where `k` = distinct prices (sort dominates).

use weakseq_types::{Batch, BatchId, ClearingResult, Fill, Price, Quantity, Side};

/// Clears a batch at a single uniform price.
#[derive(Clone, Copy, Debug, Default)]
pub struct UniformPriceAuction;

impl crate::AuctionEngine for UniformPriceAuction {
    fn clear(&self, batch: &Batch) -> ClearingResult {
        clear_uniform(batch)
    }
}

fn clear_uniform(batch: &Batch) -> ClearingResult {
    let digest = batch.digest();
    let no_trade = ClearingResult {
        batch_id: BatchId(batch.id),
        clearing_price: None,
        matched_quantity: Quantity::ZERO,
        fills: Vec::new(),
        batch_digest: digest,
    };

    if batch.orders.is_empty() {
        return no_trade;
    }

    // Candidate prices = every distinct order price.
    let mut prices: Vec<u64> = batch.orders.iter().map(|o| o.price.ticks()).collect();
    prices.sort_unstable();
    prices.dedup();

    let mut best: Option<(u64, u64, u64)> = None; // (price, matched, imbalance)
    for &p in &prices {
        let demand: u64 = batch
            .orders
            .iter()
            .filter(|o| o.side == Side::Buy && o.price.ticks() >= p)
            .map(|o| o.quantity.lots())
            .sum();
        let supply: u64 = batch
            .orders
            .iter()
            .filter(|o| o.side == Side::Sell && o.price.ticks() <= p)
            .map(|o| o.quantity.lots())
            .sum();
        let matched = demand.min(supply);
        if matched == 0 {
            continue;
        }
        let imbalance = demand.abs_diff(supply);
        match best {
            Some((_, bm, bi)) if matched < bm || (matched == bm && imbalance >= bi) => {}
            _ => best = Some((p, matched, imbalance)),
        }
    }

    let Some((price_ticks, matched, _)) = best else {
        return no_trade;
    };
    let clearing_price = Price::new(price_ticks).ok();
    let matched_qty = Quantity::new(matched).expect("matched > 0 checked above");

    // Allocate fills deterministically: eligible buys by (price desc, id asc),
    // eligible sells by (price asc, id asc), each side up to `matched`.
    let fills = allocate(batch, price_ticks, matched);

    ClearingResult {
        batch_id: BatchId(batch.id),
        clearing_price,
        matched_quantity: matched_qty,
        fills,
        batch_digest: digest,
    }
}

fn allocate(batch: &Batch, price: u64, matched: u64) -> Vec<Fill> {
    let mut fills = Vec::new();

    let mut buys: Vec<_> = batch
        .orders
        .iter()
        .filter(|o| o.side == Side::Buy && o.price.ticks() >= price)
        .collect();
    buys.sort_unstable_by(|a, b| {
        b.price
            .ticks()
            .cmp(&a.price.ticks())
            .then(a.id.0.cmp(&b.id.0))
    });

    let mut sells: Vec<_> = batch
        .orders
        .iter()
        .filter(|o| o.side == Side::Sell && o.price.ticks() <= price)
        .collect();
    sells.sort_unstable_by(|a, b| {
        a.price
            .ticks()
            .cmp(&b.price.ticks())
            .then(a.id.0.cmp(&b.id.0))
    });

    fill_side(&buys, matched, Side::Buy, &mut fills);
    fill_side(&sells, matched, Side::Sell, &mut fills);
    fills
}

fn fill_side(
    orders: &[&weakseq_types::Order],
    mut remaining: u64,
    side: Side,
    out: &mut Vec<Fill>,
) {
    for o in orders {
        if remaining == 0 {
            break;
        }
        let take = o.quantity.lots().min(remaining);
        if let Ok(q) = Quantity::new(take) {
            out.push(Fill {
                order_id: o.id,
                side,
                quantity: q,
            });
            remaining -= take;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AuctionEngine;
    use weakseq_types::Order;

    fn order(id: u64, side: Side, price: u64, qty: u64) -> Order {
        Order::new(id, side, price, qty).unwrap()
    }

    fn clear(orders: Vec<Order>) -> ClearingResult {
        UniformPriceAuction.clear(&Batch::new(1, orders))
    }

    #[test]
    fn simple_cross() {
        // Buy 5 @100, Sell 5 @90 → they cross; matched 5.
        let r = clear(vec![
            order(1, Side::Buy, 100, 5),
            order(2, Side::Sell, 90, 5),
        ]);
        assert!(!r.is_no_trade());
        assert_eq!(r.matched_quantity.lots(), 5);
        assert_eq!(r.fills.len(), 2);
    }

    #[test]
    fn no_cross_returns_no_trade() {
        // Highest buy 90 < lowest sell 100 → nothing clears.
        let r = clear(vec![
            order(1, Side::Buy, 90, 5),
            order(2, Side::Sell, 100, 5),
        ]);
        assert!(r.is_no_trade());
        assert_eq!(r.matched_quantity.lots(), 0);
        assert!(r.fills.is_empty());
    }

    #[test]
    fn partial_fill_on_imbalance() {
        // Demand 10 (two buys) vs supply 4 → matched 4.
        let r = clear(vec![
            order(1, Side::Buy, 100, 6),
            order(2, Side::Buy, 100, 4),
            order(3, Side::Sell, 95, 4),
        ]);
        assert_eq!(r.matched_quantity.lots(), 4);
        let buy_filled: u64 = r
            .fills
            .iter()
            .filter(|f| f.side == Side::Buy)
            .map(|f| f.quantity.lots())
            .sum();
        let sell_filled: u64 = r
            .fills
            .iter()
            .filter(|f| f.side == Side::Sell)
            .map(|f| f.quantity.lots())
            .sum();
        assert_eq!(buy_filled, 4);
        assert_eq!(sell_filled, 4);
    }

    #[test]
    fn clearing_is_order_independent() {
        let a = clear(vec![
            order(1, Side::Buy, 100, 5),
            order(2, Side::Sell, 90, 3),
            order(3, Side::Sell, 95, 3),
        ]);
        let b = clear(vec![
            order(3, Side::Sell, 95, 3),
            order(1, Side::Buy, 100, 5),
            order(2, Side::Sell, 90, 3),
        ]);
        assert_eq!(a.matched_quantity, b.matched_quantity);
        assert_eq!(a.clearing_price, b.clearing_price);
        assert_eq!(a.batch_digest, b.batch_digest);
    }

    #[test]
    fn empty_batch_no_trade() {
        let r = clear(vec![]);
        assert!(r.is_no_trade());
    }

    #[test]
    fn all_buys_no_trade() {
        let r = clear(vec![
            order(1, Side::Buy, 100, 5),
            order(2, Side::Buy, 90, 5),
        ]);
        assert!(r.is_no_trade());
    }

    #[test]
    fn conservation_matched_equal_both_sides() {
        let r = clear(vec![
            order(1, Side::Buy, 120, 7),
            order(2, Side::Buy, 110, 3),
            order(3, Side::Sell, 100, 6),
            order(4, Side::Sell, 105, 6),
        ]);
        let buy: u64 = r
            .fills
            .iter()
            .filter(|f| f.side == Side::Buy)
            .map(|f| f.quantity.lots())
            .sum();
        let sell: u64 = r
            .fills
            .iter()
            .filter(|f| f.side == Side::Sell)
            .map(|f| f.quantity.lots())
            .sum();
        assert_eq!(buy, sell);
        assert_eq!(buy, r.matched_quantity.lots());
    }
}
