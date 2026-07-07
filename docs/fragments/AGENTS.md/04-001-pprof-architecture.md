## `pprof` profiling architecture

The profiling pipeline is split between signal-handler-safe capture and post-sampling report generation.

- `profiler.rs` owns `ProfilerGuardBuilder` / `ProfilerGuard`, installs and uninstalls the `SIGPROF` handler, applies shared-library blocklists, and records samples into the collector.
- `timer.rs` owns the `setitimer` interval timer and `ReportTiming` metadata.
- `backtrace/` owns unwinder abstraction and backends: `backtrace_rs.rs` for the default `backtrace` crate path, `frame_pointer.rs` for frame-pointer walking, and `framehop_unwinder.rs` plus `framehop_unwinder/shlib.rs` for `framehop` unwind info.
- `collector.rs` owns the signal-conscious aggregation structures: `Collector`, `HashCounter`, `Bucket`, and temp-file-backed spill storage.
- `frames.rs` owns `UnresolvedFrames`, `Frames`, and `Symbol`; raw addresses are captured during sampling and symbolized after sampling.
- `report.rs` owns `ReportBuilder`, `UnresolvedReport`, `Report`, human-readable `Debug`, and protobuf report generation.
- `flamegraph.rs` owns the crate-local SVG flamegraph renderer behind the `flamegraph` feature.
- `addr_validate.rs` checks whether a candidate address is safely readable before backend code dereferences it.
- `perfmap.rs` behind `perfmaps` resolves JIT or dynamically generated symbols from `/tmp/perf-<pid>.map`.
- `criterion.rs` behind `criterion` wires `PProfProfiler` into Criterion's profiling hook.
- `build.rs` owns protobuf generation from `proto/profile.proto`: `protobuf-codec` writes generated code under `OUT_DIR`, while `prost-codec` refreshes the committed `proto/perftools.profiles.rs` artifact when the proto or generator config fingerprint changes.

Code reachable from `perf_signal_handler` must stay allocation-conscious, nonblocking, and panic-averse. Symbolication, demangling, report aggregation, protobuf creation, and SVG rendering happen outside that signal-handler boundary and may use ordinary owned data structures and typed errors.
