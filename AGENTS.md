# AGENTS.md

This file provides guidance to coding agents when working with code in this repository.

## What this repository is

`pprof` (crate name `pprof`, repo `strict-pprof-rs`) is a CPU sampling profiler that integrates into a running Rust program via a signal handler. It is a fork of `tikv/pprof-rs` being brought into the `strict-rs` ecosystem: the `upgrade` branch is layering in the `rust-template`-derived tooling (`xtask/`, `justfile`, `rust-toolchain.toml`) around the existing product code, which otherwise still carries its original TiKV-project history and conventions.

The product crate (`src/`) does not yet follow the panic-free/`unsafe`-forbidden policy used by the pure tooling crates elsewhere in the `strict-rs` ecosystem (see `~/strict-rs/AGENTS.md` for that ecosystem-wide policy) — signal-handler-based unwinding inherently relies on `unsafe` and there is no `clippy.toml` or `[workspace.lints]` block in `Cargo.toml` yet. Treat existing `unsafe` blocks and `unwrap()`/`expect()` calls in `src/` as pre-existing product code, not a gate violation, unless asked to change that policy.

`.github/workflows/rust.yml` is still the pre-migration CI: it pins toolchain `1.74.0`, calls `cargo fmt`/`cargo clippy`/`cargo test` directly (not through `just`/`xtask`), and runs tests with `--test-threads 1` because the signal-handler-based profiler cannot run concurrently with itself across tests in one process. It has not been updated to the `just`-based command surface described below.

## Commands

`just` is the canonical entry point (`XTASK_VIA_JUST=1` is set by the justfile itself; direct `cargo xtask <sub>` is refused otherwise unless `CI` is set). Run `just` with no arguments to list all recipes.

- `just init` — bootstrap toolchains, targets, cargo tools, and the local pre-commit hook.
- `just fmt` / `just check` / `just lint` — nightly rustfmt, type-check, and strict Clippy; each accepts an optional leading path to scope to a crate (`just lint xtask`) and forwards anything after `--` to the underlying tool.
- `just test` — nextest unit/integration tests; `just test-doc` — doctests (nextest does not run these); `just test-all` — both. A single test: `just test -- <nextest filter>`.
- `just coverage` / `just coverage-per-feature` — `cargo-llvm-cov` reports, per-file and across the feature matrix.
- `just ci` — the full local CI mirror, in CI order; `just precommit` — the pre-commit gate (also stages regenerated metrics/badges).
- `just gen-agent-guidance` — regenerates generated Markdown docs (used for `xtask/AGENTS.md` and `xtask/README.md`; this root `AGENTS.md` is hand-maintained, not generated).
- `just x <name>` — run a repo-specific extension command from `xtask/src/extensions.rs`; the registry is currently empty (no project extensions registered yet).
- `just repo-overview` — render live workspace/command/gate facts; `just agent` — agent helper reports for gates and staged changes.

Building/testing/linting non-default features needs explicit `--features`, since only `cpp` is a default feature — e.g. `just test -- --features flamegraph,prost-codec` or `just lint -- --features protobuf-codec`. `prost-codec` and `protobuf-codec` are alternative pprof-proto encoders for normal use (both flow through the internal `_protobuf` feature); aggregate feature checks choose `protobuf-codec` when both are enabled. `frame-pointer` only compiles on nightly and on `x86_64`/`aarch64`/`riscv64`/`loongarch64`; aggregate feature checks choose `framehop-unwinder` over `frame-pointer` on supported targets.

## Architecture

The profiling pipeline runs across five modules under `src/`:

- `profiler.rs` — `ProfilerGuardBuilder`/`ProfilerGuard`, the public entry point. Building a guard installs a `SIGPROF` handler (`Profiler`) driven by `timer.rs`; dropping the guard uninstalls it. The signal handler must not allocate or block, which shapes everything downstream of it.
- `backtrace/mod.rs` — the `Trace`/`Frame`/`Symbol` traits abstract over which unwinder actually walks the stack inside the signal handler. Exactly one backend is selected by `cfg`: `backtrace_rs.rs` (default, wraps the `backtrace` crate), `frame_pointer.rs` (feature `frame-pointer`, nightly-only, frame-pointer walking), or `framehop_unwinder.rs` + `framehop_unwinder/shlib.rs` (feature `framehop-unwinder`, uses the `framehop` crate plus `memmap2`/`object` for reading loaded shared-object unwind info; the `#282` change removed a per-sample allocation from this path).
- `collector.rs` — the signal-safe aggregation structures the handler writes into: `Collector<T>`/`HashCounter` (lock-free-ish counting keyed by raw stack hash) and `TempFdArray` (temp-file-backed storage), used precisely because normal heap allocation is unsafe inside a signal handler.
- `frames.rs` — `UnresolvedFrames` (raw addresses captured in-signal) versus `Frames`/`Symbol` (post-signal, demangled/resolved). Symbolication happens outside the signal handler, after sampling stops.
- `report.rs` — `ReportBuilder` drains a `Profiler` into an `UnresolvedReport` or a resolved `Report` (`HashMap<Frames, isize>`). `Report` supports a human-readable `Debug` format plus, feature-gated, `.flamegraph()`/`.flamegraph_with_options()` (via `inferno`, feature `flamegraph`) and `.pprof()` (the `perftools.profiles` protobuf, feature `prost-codec` or `protobuf-codec`).

Supporting pieces: `addr_validate.rs` checks whether an address is safely readable before the unwinder dereferences it; `perfmap.rs` (feature `perfmaps`) emits a `perf`-style map file for JIT/dynamic symbol resolution; `criterion.rs` (feature `criterion`) wires a `PProfProfiler` into `criterion`'s own per-benchmark profiling hook so a benchmark run can emit a flamegraph or protobuf profile; `MAX_DEPTH` in `lib.rs` is sized by the mutually-exclusive `large-depth`/`huge-depth` features.

`build.rs` generates the protobuf bindings from `proto/profile.proto`: under `protobuf-codec` it always regenerates via `protobuf-codegen` into `OUT_DIR`; under `prost-codec` it hashes the `.proto` file and only regenerates `proto/perftools.profiles.rs` (committed, prefixed with the hash as a comment) when the hash changes.

`xtask/` is the local automation composition crate (`strict-xtask-core`/`strict-xtask-cargo`/`strict-xtask-agents-md` plumbed together); it has its own generated `xtask/AGENTS.md` — read that before changing anything under `xtask/`.

## Commit messages

- Subject: `type(scope): structural imperative description` — conventional-commit style, scope **required**, `!` appended for breaking changes. **Never `chore`** — pick the precise type (`feat`, `fix`, `refactor`, `perf`, `build`, `ci`, `docs`, `test`, `style`, `revert`, …).
- Headline the substance: the subject names the most significant behavior/API change; renames, moves, lockfile bumps, and generated artifacts are fallout, never the headline when real behavior also changed.
- Body: 1–5 sections sized to the commit. Each section starts with a plain-text header line (no `#`, no bold), followed by 3–5 imperative bullets describing structural changes; exactly one blank line between sections; fallout goes in the last section.
- Pass the message via HEREDOC to `git commit -m` so blank lines survive shell quoting.
