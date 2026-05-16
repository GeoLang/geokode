use criterion::{Criterion, black_box, criterion_group, criterion_main};
use geokode_core::address::{Address, parse_address};
use geokode_core::geocode::GeocoderBuilder;

fn build_test_geocoder() -> geokode_core::geocode::Geocoder {
    let mut builder = GeocoderBuilder::new();
    for i in 0..10_000 {
        let addr = Address {
            house_number: Some(format!("{}", i)),
            street: Some(format!("street {}", i % 500)),
            city: Some("springfield".to_string()),
            state: Some("il".to_string()),
            postcode: Some(format!("{:05}", 60000 + i % 1000)),
            country: None,
            full: format!(
                "{} street {} springfield il {:05}",
                i,
                i % 500,
                60000 + i % 1000
            ),
        };
        let lat = 39.0 + (i as f64 / 10_000.0);
        let lon = -89.0 + (i as f64 / 10_000.0);
        builder.add(addr, lat, lon);
    }
    builder.build().unwrap()
}

fn bench_forward_geocode(c: &mut Criterion) {
    let geocoder = build_test_geocoder();
    c.bench_function("forward_geocode", |b| {
        b.iter(|| geocoder.forward(black_box("42 street 100 springfield")));
    });
}

fn bench_reverse_geocode(c: &mut Criterion) {
    let geocoder = build_test_geocoder();
    c.bench_function("reverse_geocode", |b| {
        b.iter(|| geocoder.reverse(black_box(-88.5), black_box(39.5), 5));
    });
}

fn bench_autocomplete(c: &mut Criterion) {
    let geocoder = build_test_geocoder();
    c.bench_function("autocomplete", |b| {
        b.iter(|| geocoder.autocomplete(black_box("42 street"), 10));
    });
}

fn bench_address_parsing(c: &mut Criterion) {
    c.bench_function("parse_address", |b| {
        b.iter(|| parse_address(black_box("123 Main St, Springfield, IL 62701")));
    });
}

criterion_group!(
    benches,
    bench_forward_geocode,
    bench_reverse_geocode,
    bench_autocomplete,
    bench_address_parsing
);
criterion_main!(benches);
