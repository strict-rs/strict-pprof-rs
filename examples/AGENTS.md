# examples Agent Guide

Nine runnable examples demonstrating `pprof` usage patterns. There is no `just` recipe for running an example — use `cargo run --example <name>` (add `--release` for the profiling-heavy ones) plus whatever `--features` the example needs. See `../src/AGENTS.md` for the crate's current build status before assuming an example failure is something you introduced.

## Feature requirements

The root `Cargo.toml` declares an explicit `[[example]]` stanza with `required-features` for five of the nine:

- `flamegraph.rs`, `multithread_flamegraph.rs` — `flamegraph`
- `profile_proto_with_prost.rs` — `protobuf`, `prost-codec`
- `profile_proto_with_protobuf_codec.rs` — `protobuf`, `protobuf-codec`
- `criterion.rs` — `flamegraph`, `criterion`

`backtrace_while_sampling.rs`, `multithread.rs`, `post_processor.rs`, and `prime_number.rs` have no `[[example]]` stanza, so Cargo does not enforce a feature requirement for them on the command line. `backtrace_while_sampling.rs` nonetheless calls `Report::flamegraph()`, which only compiles under the `flamegraph` feature — pass `--features flamegraph` explicitly when building or running it, since nothing else will supply it.

## What each demonstrates

- `prime_number.rs` — minimal single-threaded profiling with the default text `Debug` report.
- `multithread.rs` — profiling a workload spread across multiple threads.
- `flamegraph.rs` / `multithread_flamegraph.rs` — `Report::flamegraph()` SVG output, single- and multi-threaded.
- `post_processor.rs` — `frames_post_processor` for renaming/grouping thread names before a report is built.
- `backtrace_while_sampling.rs` — recursion plus a nested `backtrace::Backtrace::new()` call while the profiler is sampling, then a flamegraph of the result.
- `profile_proto_with_prost.rs` / `profile_proto_with_protobuf_codec.rs` — `Report::pprof()` protobuf output, one per codec feature; both use `pprof::protos::encode_profile` from the crate's `protos` module.
- `criterion.rs` — `pprof::criterion::PProfProfiler` wired into a `criterion` benchmark (fibonacci) via `Criterion::default().with_profiler(...)`.
