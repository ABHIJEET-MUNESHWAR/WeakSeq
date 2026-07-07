//! Benchmark: batch clearing time as batch size grows.

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use weakseq_auction::{AuctionEngine, UniformPriceAuction};
use weakseq_types::{Batch, Order, Side};

fn batch(n: usize) -> Batch {
    let orders = (0..n)
        .map(|i| {
            let side = if i % 2 == 0 { Side::Buy } else { Side::Sell };
            let price = 100 + (i as u64 % 20);
            Order::new(i as u64, side, price, 1 + (i as u64 % 5)).unwrap()
        })
        .collect();
    Batch::new(1, orders)
}

/// A batch where **every order has a distinct price** (k = n). This is the case
/// the old O(k·n) rescan degrades to O(n²) on; the prefix-sum clearing stays
/// O(n log n).
fn batch_unique(n: usize) -> Batch {
    let orders = (0..n)
        .map(|i| {
            let side = if i % 2 == 0 { Side::Buy } else { Side::Sell };
            let price = 100 + i as u64; // unique per order
            Order::new(i as u64, side, price, 1 + (i as u64 % 5)).unwrap()
        })
        .collect();
    Batch::new(1, orders)
}

fn bench_clearing(c: &mut Criterion) {
    let mut group = c.benchmark_group("clearing");
    for n in [100usize, 1_000, 10_000] {
        let b = batch(n);
        group.throughput(criterion::Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &b, |bh, b| {
            bh.iter(|| UniformPriceAuction.clear(b));
        });
    }
    group.finish();

    let mut unique = c.benchmark_group("clearing_unique");
    for n in [100usize, 1_000, 10_000] {
        let b = batch_unique(n);
        unique.throughput(criterion::Throughput::Elements(n as u64));
        unique.bench_with_input(BenchmarkId::from_parameter(n), &b, |bh, b| {
            bh.iter(|| UniformPriceAuction.clear(b));
        });
    }
    unique.finish();
}

criterion_group!(benches, bench_clearing);
criterion_main!(benches);
