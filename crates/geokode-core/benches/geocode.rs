use criterion::{Criterion, criterion_group, criterion_main};
use geokode_core::address::{Address, FeatureKind, parse_address};
use geokode_core::geocode::Geocoder;
use geokode_core::index::{IndexWriter, PreparedRecord, Record};
use std::hint::black_box;

fn build_test_geocoder(directory: &std::path::Path) -> Geocoder {
    let mut writer = IndexWriter::create(directory).unwrap();
    for i in 0..10_000 {
        let lat = 39.0 + (f64::from(i) / 10_000.0);
        let lon = -89.0 + (f64::from(i) / 10_000.0);
        let mut record = Record::new(FeatureKind::Address, lon, lat);
        record.address = Address {
            house_number: Some(format!("{i}")),
            street: Some(format!("street {}", i % 500)),
            city: Some("springfield".to_string()),
            ..Address::default()
        };
        writer
            .add(PreparedRecord::new(record), Vec::new(), false)
            .unwrap();
    }
    writer.finish().unwrap();
    Geocoder::open(directory).unwrap()
}

fn bench_geocoder(c: &mut Criterion) {
    let directory = tempfile::tempdir().unwrap();
    let geocoder = build_test_geocoder(directory.path());
    c.bench_function("forward_geocode", |b| {
        b.iter(|| geocoder.forward(black_box("42 street 42"), 5, None));
    });
    c.bench_function("reverse_geocode", |b| {
        b.iter(|| geocoder.reverse(black_box(-88.9), black_box(39.1), 5));
    });
    c.bench_function("autocomplete", |b| {
        b.iter(|| geocoder.autocomplete(black_box("42 street"), 10, None));
    });
}

fn bench_address_parsing(c: &mut Criterion) {
    c.bench_function("parse_address", |b| {
        b.iter(|| parse_address(black_box("123 Main St, Springfield, IL 62701")));
    });
}

criterion_group!(benches, bench_geocoder, bench_address_parsing);
criterion_main!(benches);
