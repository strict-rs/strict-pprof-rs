# benches Agent Guide

Two `criterion` benchmark suites (`harness = false`), each registered as a `[[bench]]` entry in the root `Cargo.toml`. Run with plain `cargo bench` (`cargo bench --bench collector` / `cargo bench --bench addr_validate` for a single suite) — there is no `just` recipe wrapping benches. Both build with default features only; neither depends on any optional feature. See `../src/AGENTS.md` for the crate's current build status before assuming a benchmark failure is something you introduced.

- `collector.rs` — benchmarks `Collector::add` and `HashCounter::add` from `../src/collector.rs`, the signal-safe sample-aggregation structures the profiler's signal handler writes into.
- `addr_validate.rs` — benchmarks `pprof::validate` (`../src/addr_validate.rs`) against stack and heap addresses, the check the unwinder runs before dereferencing a candidate frame address.
