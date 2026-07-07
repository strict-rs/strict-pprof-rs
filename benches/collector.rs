// Copyright 2019 TiKV Project Authors. Licensed under Apache-2.0.

use criterion::Criterion;
use criterion::criterion_group;
use criterion::criterion_main;
use pprof::Collector;
use pprof::HashCounter;

#[path = "common/inputs.rs"]
mod inputs;

fn bench_write_to_collector(c: &mut Criterion) {
  c.bench_function("write_to_collector", |b| {
    let mut collector = Collector::new().unwrap();
    let samples = inputs::random_u64_values();

    b.iter(|| {
      samples.iter().for_each(|sample| {
        collector.add(*sample, 1).unwrap();
      })
    })
  });

  c.bench_function("write_into_stack_hash_counter", |b| {
    let mut collector = HashCounter::default();
    let samples = inputs::random_u64_values();

    b.iter(|| {
      samples.iter().for_each(|sample| {
        collector.add(*sample, 1);
      })
    });
  });
}

criterion_group!(benches, bench_write_to_collector);
criterion_main!(benches);
