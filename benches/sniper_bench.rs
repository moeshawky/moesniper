use criterion::{criterion_group, criterion_main, Criterion};

fn my_benchmark(c: &mut Criterion) {
    let text = "a\n".repeat(10000);
    c.bench_function("split_inclusive_to_string", |b| b.iter(|| {
        let lines: Vec<String> = std::hint::black_box(&text).split_inclusive('\n').map(String::from).collect();
        std::hint::black_box(lines);
    }));

    c.bench_function("split_inclusive_to_str", |b| b.iter(|| {
        let lines: Vec<&str> = std::hint::black_box(&text).split_inclusive('\n').collect();
        std::hint::black_box(lines);
    }));
}

criterion_group!(benches, my_benchmark);
criterion_main!(benches);
