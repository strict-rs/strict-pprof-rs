## `xtask` role in `strict-pprof-rs`

`xtask` is the repository-specific extension crate for the `pprof` product. Standard workflows run through the installed `template` binary; this crate owns only the guarded local `x` registry.

- Keep reusable workflow behavior in its owning `template-rs` crate, reusable production effects in `strict-standard`, and reusable test effects in `strict-test-support`.
- Keep repo-specific automation behind `just x <name>` through the extension registry when the product genuinely needs local behavior.
- Keep local protobuf automation under `just x proto` (`setup`, `update`, `gen`) and protobuf/prost verification under `just x matrix`; share the pinned `protoc` setup path instead of duplicating toolchain orchestration.
- Preserve the current `[lints] workspace = true` opt-in for `xtask`; do not use `xtask` lint compliance as permission to opt the main `pprof` crate into workspace lints in the same wave.
- Use `just` for repository workflows. Direct `cargo xtask x` remains an implementation detail of the guarded local extension recipe, not a standard command surface.
