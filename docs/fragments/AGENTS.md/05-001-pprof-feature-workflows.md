## `pprof` feature semantics

Only `cpp` is enabled by default. Feature work must preserve the intended precedence across report codecs and unwinder backends.

- `cpp` enables C++ demangling through `symbolic-demangle/cpp`; without it, C++ symbols remain mangled while Rust demangling still works.
- `flamegraph` enables the crate-owned SVG renderer through the permissive `svg` dependency. Do not reintroduce `inferno` or any `CDDL-1.0` dependency path.
- `prost-codec` enables `prost`, `prost-build`, `sha2`, and `_protobuf`; generated prost bindings still derive `::prost::Message` through `prost`, not a direct root `prost-derive` dependency.
- `protobuf-codec` enables `protobuf`, `protobuf-codegen`, and `_protobuf`.
- `_protobuf` is private plumbing for report generation shared by the two public codec features. When both codec features are present, the crate uses the `protobuf-codec` generated module.
- `framehop-unwinder` enables `framehop`, `memmap2`, and `object`; where supported it takes precedence over `frame-pointer`.
- `frame-pointer` enables the frame-pointer backend for supported architectures when `framehop-unwinder` is not selected.
- `perfmaps` enables dynamic symbol lookup through `arc-swap` and `/tmp/perf-<pid>.map`.
- `large-depth` and `huge-depth` change `MAX_DEPTH`; do not enable both intentionally.

Run the relevant feature command when changing feature declarations, generated protobuf bindings, flamegraph behavior, or backend selection: `just hack` for the matrix, `cargo check --features flamegraph --locked`, `cargo check --features prost-codec --locked`, and `cargo check --features protobuf-codec --locked` for focused confirmation.
