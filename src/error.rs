// Copyright 2019 TiKV Project Authors. Licensed under Apache-2.0.

use std::os::raw::c_int;

#[derive(Debug, thiserror::Error)]
pub enum Error {
  #[error("failed to update SIGPROF signal handler: {0}")]
  SignalHandler(#[from] nix::Error),
  #[error("failed to store collected profiler samples: {0}")]
  SampleStorage(#[from] std::io::Error),
  #[error("profiler failed to initialize")]
  ProfilerInitialization,
  #[error("failed to ensure perf map file: {0}")]
  PerfMapFile(#[source] std::io::Error),
  #[error("invalid profiler frequency {0}; expected a positive sampling frequency")]
  InvalidFrequency(c_int),
  #[error("profiler is already running")]
  AlreadyRunning,
  #[error("profiler is not running")]
  NotRunning,
  #[error("invalid flamegraph options: {0}")]
  InvalidFlamegraphOptions(String),
  #[error("failed to write flamegraph SVG: {0}")]
  FlamegraphOutput(#[source] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
  use std::error::Error as _;

  use strict_test_support::TestFailure;
  use strict_test_support::ensure_all;

  use super::Error;

  fn io_source_kind(error: &Error) -> Option<std::io::ErrorKind> {
    error
      .source()
      .and_then(|source| source.downcast_ref::<std::io::Error>())
      .map(std::io::Error::kind)
  }

  #[test]
  fn domain_errors_expose_stable_messages_without_sources() -> std::result::Result<(), TestFailure> {
    let profiler_initialization = Error::ProfilerInitialization;
    let invalid_frequency = Error::InvalidFrequency(0);
    let already_running = Error::AlreadyRunning;
    let not_running = Error::NotRunning;
    let invalid_flamegraph_options = Error::InvalidFlamegraphOptions("frame_height must be greater than zero".to_owned());

    ensure_all(&[
      (
        profiler_initialization.to_string() == "profiler failed to initialize",
        "profiler initialization error should describe profiler setup failure",
      ),
      (
        invalid_frequency.to_string() == "invalid profiler frequency 0; expected a positive sampling frequency",
        "invalid frequency error should include rejected frequency",
      ),
      (
        already_running.to_string() == "profiler is already running",
        "already-running error should describe duplicate start failure",
      ),
      (
        not_running.to_string() == "profiler is not running",
        "not-running error should describe inactive stop failure",
      ),
      (
        invalid_flamegraph_options.to_string() == "invalid flamegraph options: frame_height must be greater than zero",
        "flamegraph option errors should include the invalid option message",
      ),
      (
        profiler_initialization.source().is_none(),
        "profiler initialization errors should not expose a lower-level source error",
      ),
      (
        invalid_frequency.source().is_none(),
        "invalid frequency errors should not expose a lower-level source error",
      ),
      (
        already_running.source().is_none(),
        "already-running errors should not expose a lower-level source error",
      ),
      (
        not_running.source().is_none(),
        "not-running errors should not expose a lower-level source error",
      ),
      (
        invalid_flamegraph_options.source().is_none(),
        "invalid flamegraph option errors should not expose a lower-level source error",
      ),
    ])
  }

  #[test]
  fn wrapped_errors_preserve_display_and_source_context() -> std::result::Result<(), TestFailure> {
    let signal_error = Error::from(nix::errno::Errno::EINVAL);
    let sample_storage_error = Error::from(std::io::Error::from(std::io::ErrorKind::NotFound));
    let perf_map_file_error = Error::PerfMapFile(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
    let flamegraph_output_error = Error::FlamegraphOutput(std::io::Error::from(std::io::ErrorKind::BrokenPipe));

    ensure_all(&[
      (
        signal_error.to_string().contains("failed to update SIGPROF signal handler"),
        "signal-handler display should name SIGPROF setup",
      ),
      (
        signal_error.to_string().contains("EINVAL"),
        "signal-handler display should preserve the underlying errno",
      ),
      (
        signal_error.source().is_some(),
        "signal-handler error conversion should preserve source error",
      ),
      (
        sample_storage_error
          .to_string()
          .contains("failed to store collected profiler samples"),
        "sample-storage display should name sample storage",
      ),
      (
        io_source_kind(&sample_storage_error) == Some(std::io::ErrorKind::NotFound),
        "sample-storage error conversion should preserve source context",
      ),
      (
        perf_map_file_error.to_string().contains("failed to ensure perf map file"),
        "perf-map display should name perf map setup",
      ),
      (
        io_source_kind(&perf_map_file_error) == Some(std::io::ErrorKind::PermissionDenied),
        "perf-map file errors should preserve source context",
      ),
      (
        flamegraph_output_error.to_string().contains("failed to write flamegraph SVG"),
        "flamegraph display should name SVG output",
      ),
      (
        io_source_kind(&flamegraph_output_error) == Some(std::io::ErrorKind::BrokenPipe),
        "flamegraph output errors should preserve source context",
      ),
    ])
  }
}
