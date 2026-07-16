// Copyright 2019 TiKV Project Authors. Licensed under Apache-2.0.

//! pprof-rs is an integrated profiler for rust program.
//!
//! This crate provides a programable interface to start/stop/report a profiler
//! dynamically. With the help of this crate, you can easily integrate a
//! profiler into your rust program in a modern, convenient way.
//!
//! A sample usage is:
//!
//! ```rust
//! let guard = pprof::ProfilerGuard::new(100).unwrap();
//! ```
//!
//! Then you can read report from the guard:
//!
//! ```rust
//! # let guard = pprof::ProfilerGuard::new(100).unwrap();
//! if let Ok(report) = guard.report().build() {
//!   println!("report: {:?}", &report);
//! };
//! ```
//!
//! More configuration can be passed through `ProfilerGuardBuilder`:
//!
//! ```rust
//! let guard = pprof::ProfilerGuardBuilder::default()
//!   .frequency(1000)
//!   .blocklist(&["libc", "libgcc", "pthread", "vdso"])
//!   .build()
//!   .unwrap();
//! ```
//!
//! The frequency means the sampler frequency, and the `blocklist` means the
//! profiler will ignore the sample whose first frame is from library containing
//! these strings.
//!
//! Skipping `libc`, `libgcc` and `libpthread` could be a solution to the
//! possible deadlock inside the `_Unwind_Backtrace`, and keep the signal
//! safety. The dwarf information in "vdso" is incorrect in some distributions,
//! so it's also suggested to skip it.
//!
//! You can find more details in
//! [README.md](https://github.com/tikv/pprof-rs/blob/master/README.md)

/// Define the MAX supported stack depth. TODO: make this variable mutable.
#[cfg(feature = "large-depth")]
pub const MAX_DEPTH: usize = 1024;

#[cfg(all(feature = "huge-depth", not(feature = "large-depth")))]
pub const MAX_DEPTH: usize = 512;

#[cfg(not(any(feature = "large-depth", feature = "huge-depth")))]
pub const MAX_DEPTH: usize = 128;

/// Define the MAX supported thread name length. TODO: make this variable mutable.
pub const MAX_THREAD_NAME: usize = 16;

mod addr_validate;

mod backtrace;
mod collector;
mod error;
#[cfg(feature = "flamegraph")]
pub mod flamegraph;
mod frames;
#[cfg(feature = "perfmaps")]
mod perfmap;
mod profiler;
mod report;
mod timer;

pub use self::addr_validate::validate;
pub use self::collector::Collector;
pub use self::collector::HashCounter;
pub use self::error::Error;
pub use self::error::Result;
pub use self::frames::Frames;
pub use self::frames::Symbol;
pub use self::profiler::ProfilerGuard;
pub use self::profiler::ProfilerGuardBuilder;
pub use self::report::Report;
pub use self::report::ReportBuilder;
pub use self::report::UnresolvedReport;

#[cfg(all(feature = "prost-codec", not(feature = "protobuf-codec")))]
pub mod protos {
  pub use prost::Message;

  include!(concat!(env!("CARGO_MANIFEST_DIR"), "/proto/perftools.profiles.rs"));

  pub type EncodeError = prost::EncodeError;

  pub fn encode_profile(profile: &Profile) -> std::result::Result<Vec<u8>, EncodeError> {
    let mut content = Vec::new();
    profile.encode(&mut content)?;
    Ok(content)
  }
}

#[cfg(feature = "protobuf-codec")]
pub mod protos {
  pub use protobuf::Message;
  pub use protobuf::Serialize;
  pub use protobuf::prelude::*;

  include!(concat!(env!("OUT_DIR"), "/protobuf_generated/proto/generated.rs"));

  pub type EncodeError = protobuf::SerializeError;

  pub fn encode_profile(profile: &Profile) -> std::result::Result<Vec<u8>, EncodeError> {
    profile.serialize()
  }
}

#[cfg(feature = "criterion")]
pub mod criterion;

#[cfg(test)]
mod tests {
  use strict_test_support::TestFailure;
  use strict_test_support::ensure_eq;

  use super::*;

  #[test]
  fn max_thread_name_matches_fixed_profiler_buffer_contract() -> std::result::Result<(), TestFailure> {
    ensure_eq(
      &MAX_THREAD_NAME,
      &16,
      "thread-name buffer width should match the profiler's fixed native-name contract",
    )
  }

  #[test]
  fn max_depth_matches_enabled_feature_contract() -> std::result::Result<(), TestFailure> {
    #[cfg(feature = "large-depth")]
    ensure_eq(&MAX_DEPTH, &1024, "large-depth feature should select 1024 stack frames")?;
    #[cfg(all(feature = "huge-depth", not(feature = "large-depth")))]
    ensure_eq(&MAX_DEPTH, &512, "huge-depth feature should select 512 stack frames")?;
    #[cfg(not(any(feature = "large-depth", feature = "huge-depth")))]
    ensure_eq(&MAX_DEPTH, &128, "default stack depth should be 128 frames")?;
    Ok(())
  }
}
