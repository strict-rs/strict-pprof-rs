# proto Agent Guide

- `profile.proto` — the `perftools.profiles` pprof schema (Google's public pprof profile format). Consumed by the root `build.rs`.
- `perftools.profiles.rs` — committed, `prost`-generated bindings used by the `prost-codec` feature. Its first line is a `// <sha256>  proto/profile.proto` comment; `build.rs`'s prost codegen path only regenerates this file when that hash no longer matches `profile.proto`'s current contents. **Do not hand-edit `perftools.profiles.rs`** — edit `profile.proto` and let the build regenerate it (removing the hash-comment line, or the file itself, forces regeneration on the next build).

The `protobuf-codec` feature does not use `perftools.profiles.rs` at all: `build.rs`'s `protobuf-codegen` path regenerates fresh bindings on every build under `OUT_DIR/protobuf_generated/proto/` instead of reusing a committed file. It requires the exact `protoc 35.1` compiler supplied through repo-local `proto` configuration in `.prototools`; do not fall back to system `apt`/`brew` `protoc` when checking this feature.

CI's lint job removes the generated `.rs` files before running Clippy under `prost-codec` specifically to force regeneration, then diffs the result — a stale committed `perftools.profiles.rs` fails that check.
