use criterion::{Criterion, criterion_group, criterion_main};
use geokode_core::address::{normalize_street, parse_address};
use geokode_core::spatial::{SpatialIndex, SpatialRecord};
use std::hint::black_box;

fn bench_address_parse(c: &mut Criterion) {
    c.bench_function("parse_address", |b| {
        b.iter(|| parse_address(black_box("10 Downing Street, London, SW1A 2AA")))
    });
}

fn bench_normalize_street(c: &mut Criterion) {
    c.bench_function("normalize_street", |b| {
        b.iter(|| normalize_street(black_box("North Michigan Avenue")))
    });
}

fn bench_spatial_nearest(c: &mut Criterion) {
    let records: Vec<SpatialRecord> = (0..1000)
        .map(|i| SpatialRecord {
            id: i,
            lon: -0.1 + (i as f64) * 0.001,
            lat: 51.5 + (i as f64) * 0.0001,
        })
        .collect();
    let index = SpatialIndex::build(records);

    c.bench_function("spatial_nearest_1000", |b| {
        b.iter(|| index.nearest(black_box(-0.05), black_box(51.55), black_box(5)))
    });
}

criterion_group!(
    benches,
    bench_address_parse,
    bench_normalize_street,
    bench_spatial_nearest
);
criterion_main!(benches);
