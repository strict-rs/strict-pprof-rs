## `xtask` role in `strict-pprof-rs`

`xtask` is the local automation composition crate for the `pprof` product repository. It wires shared `strict-xtask-*` command providers into the repo's `just` surface and should stay thin.

- Keep reusable workflow behavior in the owning shared `strict-xtask-*` crate, not in this repository's `xtask`.
- Keep repo-specific automation behind `just x <name>` through the extension registry when the product genuinely needs local behavior.
- Preserve the current `[lints] workspace = true` opt-in for `xtask`; do not use `xtask` lint compliance as permission to opt the main `pprof` crate into workspace lints in the same wave.
- Use `just`, not direct `cargo xtask`, when running workflows from this repository.
