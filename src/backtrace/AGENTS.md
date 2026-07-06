# src/backtrace Agent Guide

This directory defines the stack-unwinding abstraction used by the profiler's `SIGPROF` handler, and the three interchangeable backends that implement it. Everything here runs inside the signal handler under the same no-allocation/no-blocking constraint described in `../AGENTS.md` — none of these backends may allocate, lock, or panic on the sampled path.

## Trait abstraction

`mod.rs` defines `Symbol`, `Frame`, and `Trace`. Exactly one backend is compiled in as `TraceImpl`, chosen by target architecture, target OS, and which of the `frame-pointer`/`framehop-unwinder` features are enabled. In aggregate feature builds, `framehop-unwinder` takes precedence over `frame-pointer` on supported targets; otherwise `frame-pointer` takes precedence over `backtrace_rs`. Check `mod.rs`'s backend selection before assuming a change to one backend applies to a given build; the three are mutually exclusive at compile time, never compiled together.

## Backends

- `backtrace_rs.rs` — the default/fallback backend, wrapping the `backtrace` crate. Compiled whenever neither `frame-pointer` nor `framehop-unwinder` applies to the current target.
- `frame_pointer.rs` — feature `frame-pointer`, nightly-only, `x86_64`/`aarch64`/`riscv64`/`loongarch64` only. Walks the stack via frame pointers instead of DWARF/CFI unwind tables.
- `framehop_unwinder.rs` plus `framehop_unwinder/shlib.rs` — feature `framehop-unwinder`, `x86_64`/`aarch64` on `linux`/`macos` only. Built on the `framehop` crate; `shlib.rs` maps the process's loaded shared objects (via `memmap2`/`object`) so `framehop` can look up unwind info for each. This backend avoids allocating on the per-sample path.

## Symbol resolution

`mod.rs` implements the crate's `Symbol` trait directly for `backtrace::Symbol`, which is what `backtrace_rs.rs` resolves through. Regardless of backend, unwinding inside the handler only needs raw addresses/frame pointers; demangled symbolication happens later via `../frames.rs`, outside the signal handler.
