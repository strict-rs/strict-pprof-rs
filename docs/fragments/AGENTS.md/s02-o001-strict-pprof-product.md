## `pprof` product identity

This repository packages the `pprof` crate: a CPU sampling profiler for Rust programs that installs a `SIGPROF` signal handler, captures stack samples from a running process, and turns those samples into report formats. The repository is a fork of `tikv/pprof-rs` being aligned with the `strict-rs` ecosystem while preserving the profiler product surface.

- Treat `src/` as the primary product crate in normal work. Template-managed automation supports the product; it does not define profiler behavior.
- Keep the flat workspace layout declared in `[ecosystems.cargo.layout]` in `template.config.toml`: the root crate is the product package and `stask/` is the local automation package.
- Preserve existing public examples and feature surfaces unless the user explicitly retires them. Example files are product documentation, not disposable duplication fodder.
- Frame commits around product/API behavior first. Template-update artifacts, generated guidance, coverage reports, and policy files are supporting fallout unless the task is explicitly about automation.

Strict migration is staged. `stask` is opted into workspace lints now; the main `pprof` crate is not opted into `[lints] workspace = true` in the current wave. Do not enable the main-crate lint opt-in opportunistically while clearing current gates; plan that as a separate user-approved wave.
