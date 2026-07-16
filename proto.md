### Upgrade `protobuf-codec` To `protobuf` 4.35.1

#### Summary

- Upgrade `protobuf` and `protobuf-codegen` together from `3.7.2` to `4.35.1-release`.
- Keep `protobuf-codec` as a build-generated `OUT_DIR` path, but provision exact `protoc 35.1` through `proto` instead of system `apt`/`brew` `protoc`.
- Preserve current public feature behavior: `protobuf-codec` still wins over `prost-codec`, `Report::pprof()` still returns `pprof::protos::Profile`, and the existing protobuf examples/Criterion output still write `profile.pb`.

#### Key Changes

- Add a local `.prototools` file that pins exact `protoc = "=35.1"` and configures the `protoc` plugin with the pinned raw plugin URL:
  ```toml
  protoc = "=35.1"

  [plugins.tools]
  protoc = "https://raw.githubusercontent.com/b4nst/proto-plugins/6574ad30cb7bd59b3429b0e2c51fe245d350ef70/toml/protoc.toml"
  ```
- Update `Cargo.toml` workspace dependencies:
  ```toml
  protobuf = "4.35.1-release"
  protobuf-codegen = "4.35.1-release"
  ```
  Then refresh `Cargo.lock` with the resolved v4 graph.
- Update `build.rs` `generate_protobuf()` from the v3 `protobuf_codegen::Codegen`/`pure()` API to v4 `protobuf_codegen::CodeGen`:
  - Use `.input("proto/profile.proto")`, `.include(".")`, `.output_dir(<OUT_DIR>/protobuf_generated)`, and `.generate_and_compile()`.
  - Stop creating `OUT_DIR/mod.rs`; v4 codegen emits `OUT_DIR/protobuf_generated/proto/profile.u.pb.rs` plus the wrapper module `OUT_DIR/protobuf_generated/proto/generated.rs`.
  - Emit `cargo:rerun-if-env-changed=PROTOC` because `protobuf-codegen` honors `PROTOC`.
- Update `src/lib.rs` `protos` module under `protobuf-codec`:
  - Include `concat!(env!("OUT_DIR"), "/protobuf_generated/proto/generated.rs")`, because the generated `profile.u.pb.rs` file references sibling messages through the wrapper module.
  - Re-export `protobuf::prelude::*` for v4 trait methods.
  - Add a small public codec-neutral helper, e.g. `protos::encode_profile(&Profile) -> Result<Vec<u8>, EncodeError>`, implemented with `prost::Message::encode` for `prost-codec` and `protobuf::Serialize::serialize` for `protobuf-codec`.
- Update `src/report.rs` protobuf-v4 constructors:
  - Keep the current `new_function`, `new_line`, `new_location`, `new_label`, `new_sample`, `new_value_type`, and `new_profile` helper boundary.
  - For `protobuf-codec`, replace public-field struct literals with v4 `::new()`, scalar setters such as `set_id`/`set_name`, message setters such as `set_period_type`, and generated repeated setters fed with iterators such as `.into_iter()` or `std::iter::once(...)`.
  - Keep the `prost-codec` branch using direct prost struct fields.
- Update encoding callers:
  - In `src/criterion.rs`, replace feature-specific `profile.encode(...)` / `profile.write_to_vec(...)` logic with `crate::protos::encode_profile(&profile)`.
  - In `examples/common/profile_proto.rs`, use `pprof::protos::encode_profile(&profile)` for both `run_prost_profile()` and `run_protobuf_profile()`.
  - Update `README.md` sample code to use the helper instead of `profile.encode(...)`, which is not valid for both codecs.
- Update docs/guidance:
  - Update `proto/AGENTS.md` to mention that `protobuf-codec` now requires `protoc 35.1` via `proto` and emits `profile.u.pb.rs` under `OUT_DIR`.
  - Update `examples/AGENTS.md` to say examples use `pprof::protos::encode_profile`, not `pprof::protos::Message`.
  - Update `docs/fragments/AGENTS.md/04-001-pprof-architecture.md` and `05-001-pprof-feature-workflows.md`, then run `just gen-agent-guidance` so generated `AGENTS.md` stays in sync.
- Update `.github/workflows/rust.yml` protobuf steps:
  - Replace `sudo apt-get install -y protobuf-compiler` with `proto` setup plus `proto install protoc`.
  - Export the installed `protoc` path for Cargo with `PROTOC=$(proto --reporter text bin protoc)` in `$GITHUB_ENV`, and add proto shims/bin paths to `$GITHUB_PATH`.
  - Apply this setup before any `protobuf-codec` clippy/build/test step on Ubuntu and macOS matrix jobs.

#### Test Plan

- Prepare the local toolchain before protobuf checks:
  ```bash
  proto install protoc
  PROTOC="$(proto --reporter text bin protoc)"
  "$PROTOC" --version
  ```
  The required output is `libprotoc 35.1`; do not use `/usr/bin/protoc 3.21.12`.
- Verify focused codec builds:
  ```bash
  cargo check --features protobuf-codec --locked
  cargo check --features prost-codec --locked
  cargo check --features flamegraph,protobuf-codec --all-targets --locked
  cargo check --features flamegraph,prost-codec --all-targets --locked
  ```
- Verify aggregate behavior and examples:
  ```bash
  cargo check --workspace --all-features --all-targets --locked
  cargo clippy --workspace --all-features --all-targets -- -D warnings
  cargo test --all-targets --workspace
  cargo run --example profile_proto_with_protobuf_codec --features protobuf-codec
  cargo run --example profile_proto_with_prost --features prost-codec
  ```
- Verify dependency hygiene:
  ```bash
  cargo tree --duplicates --locked --target all --all-features --no-default-features
  just audit
  ```
  Expected outcome: the old `protobuf` v3 cluster (`protobuf-parse`, `which`, `thiserror 1`, `rustix 0.38`, `linux-raw-sys 0.4`, `windows-sys 0.59`) disappears or shrinks; remaining upstream duplicates from `backtrace`, `criterion`, `prost-build`, or `framehop` remain warning-level unless this upgrade genuinely resolves them.
- Verify generated guidance/artifacts:
  ```bash
  just gen-agent-guidance
  git diff --check
  ```
  Review generated Markdown diffs rather than hand-editing generated `AGENTS.md`.

#### Assumptions

- Use `proto` as the project’s `protoc` provider; do not add `protoc-bin-vendored`, `dlprotoc`, or a build-time downloader.
- Keep `protobuf-codec` generated under `OUT_DIR`, not committed into `proto/`.
- Do not add lint suppressions around generated code. If v4 generated output itself trips deny-level linting under the repo’s real clippy command, stop and report the exact generated warning instead of hiding it.
- The relevant external references are: `protobuf-codegen 4.35.1-release` requires matching `protoc` and exposes `CodeGen::generate_and_compile()`; official protobuf Rust generated code uses setters/accessors and `serialize()` through traits; `proto` project config supports local `.prototools` pins and plugin configuration. Sources: [protobuf-codegen crate](https://crates.io/crates/protobuf-codegen), [protobuf Rust generated code guide](https://protobuf.dev/reference/rust/rust-generated/), [proto configuration docs](https://moonrepo.dev/docs/proto/config), [proto plugins docs](https://moonrepo.dev/docs/proto/plugins), [pinned protoc proto plugin](https://raw.githubusercontent.com/b4nst/proto-plugins/6574ad30cb7bd59b3429b0e2c51fe245d350ef70/toml/protoc.toml).
