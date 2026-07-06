#[macro_use]
extern crate criterion;
use criterion::BenchmarkId;
use criterion::Criterion;
use pprof::criterion::Output;
use pprof::criterion::PProfProfiler;

// Thanks to the example provided by @jebbow in his article
// https://www.jibbow.com/posts/criterion-flamegraphs/

fn fibonacci(n: u64) -> u64 {
  match n {
    0 | 1 => 1,
    n => fibonacci(n - 1) + fibonacci(n - 2),
  }
}

fn bench(c: &mut Criterion) {
  c.bench_function("Fibonacci", |b| b.iter(|| fibonacci(std::hint::black_box(20))));
}

fn bench_group(c: &mut Criterion) {
  let mut group = c.benchmark_group("Fibonacci Sizes");

  for s in &[1, 10, 100, 1000] {
    group.bench_with_input(BenchmarkId::from_parameter(s), s, |b, s| {
      b.iter(|| fibonacci(std::hint::black_box(*s)))
    });
  }
}

criterion_group! {
    name = benches;
    config = Criterion::default().with_profiler(PProfProfiler::new(100, Output::Flamegraph(None)));
    targets = bench, bench_group
}
criterion_main!(benches);
