// Copyright 2019 TiKV Project Authors. Licensed under Apache-2.0.

use std::error::Error;

use pprof::Report;

use crate::prime;

pub type ExampleResult<T> = Result<T, Box<dyn Error>>;

#[cfg(feature = "flamegraph")]
pub fn run_flamegraph_profile() -> ExampleResult<()> {
  let (prime_count, report) = collect_partitioned_report()?;
  log_prime_count(prime_count);
  write_flamegraph(&report)?;
  log_report(&report);
  Ok(())
}

#[cfg(feature = "prost-codec")]
pub fn run_prost_profile() -> ExampleResult<()> {
  let (prime_count, report) = collect_partitioned_report()?;
  log_prime_count(prime_count);

  write_encoded_profile(&report)?;

  log_report(&report);
  Ok(())
}

#[cfg(feature = "protobuf-codec")]
pub fn run_protobuf_profile() -> ExampleResult<()> {
  let (prime_count, report) = collect_partitioned_report()?;
  log_prime_count(prime_count);

  write_encoded_profile(&report)?;

  log_report(&report);
  Ok(())
}

pub fn collect_partitioned_report() -> ExampleResult<(usize, Report)> {
  let prime_numbers = prime::prepare_prime_numbers();
  let guard = pprof::ProfilerGuard::new(100)?;
  let prime_count = prime::count_partitioned_primes(5_000_000, &prime_numbers);
  let report = guard.report().build()?;
  Ok((prime_count, report))
}

#[cfg(any(feature = "prost-codec", feature = "protobuf-codec"))]
fn write_encoded_profile(report: &Report) -> ExampleResult<()> {
  let profile = report.pprof()?;
  let content = pprof::protos::encode_profile(&profile)?;
  write_profile_bytes(&content)
}

#[cfg(any(feature = "prost-codec", feature = "protobuf-codec"))]
pub fn write_profile_bytes(content: &[u8]) -> ExampleResult<()> {
  let mut file = std::fs::File::create("profile.pb")?;
  std::io::Write::write_all(&mut file, content)?;
  Ok(())
}

#[cfg(feature = "flamegraph")]
pub fn write_flamegraph(report: &Report) -> ExampleResult<()> {
  let file = std::fs::File::create("flamegraph.svg")?;
  report.flamegraph(file)?;
  Ok(())
}

pub fn log_prime_count(prime_count: usize) {
  log::info!("Prime numbers: {prime_count}");
}

pub fn log_report(report: &Report) {
  log::info!("report: {report:?}");
}

#[cfg(all(test, any(feature = "prost-codec", feature = "protobuf-codec")))]
mod tests {
  use parking_lot::Mutex;
  use strict_test_support::TestFailure;
  use strict_test_support::ensure;
  use strict_test_support::ensure_ok;

  use super::*;

  static CURRENT_DIR_LOCK: Mutex<()> = Mutex::new(());

  struct CurrentDirGuard {
    previous: std::path::PathBuf,
  }

  impl CurrentDirGuard {
    fn enter(path: &std::path::Path) -> std::result::Result<Self, TestFailure> {
      let previous = ensure_ok(std::env::current_dir(), "current directory should be readable")?;
      ensure_ok(std::env::set_current_dir(path), "test current directory should be entered")?;
      Ok(Self {
        previous,
      })
    }
  }

  impl Drop for CurrentDirGuard {
    fn drop(&mut self) {
      let _ = std::env::set_current_dir(&self.previous);
    }
  }

  #[test]
  fn write_profile_bytes_writes_profile_pb_in_current_directory() -> std::result::Result<(), TestFailure> {
    let _guard = CURRENT_DIR_LOCK.lock();
    let dir = ensure_ok(tempfile::tempdir(), "tempdir should be created")?;
    let _current_dir = CurrentDirGuard::enter(dir.path())?;

    ensure(
      write_profile_bytes(b"profile-bytes").is_ok(),
      "profile bytes helper should write without error",
    )?;

    let profile = ensure_ok(std::fs::read("profile.pb"), "profile output should be readable")?;
    ensure(
      profile == b"profile-bytes",
      "profile bytes helper should write content to profile.pb",
    )
  }

  #[test]
  fn write_profile_bytes_reports_error_when_profile_path_is_directory() -> std::result::Result<(), TestFailure> {
    let _guard = CURRENT_DIR_LOCK.lock();
    let dir = ensure_ok(tempfile::tempdir(), "tempdir should be created")?;
    let _current_dir = CurrentDirGuard::enter(dir.path())?;
    ensure_ok(std::fs::create_dir("profile.pb"), "directory should occupy profile output path")?;

    ensure(
      write_profile_bytes(b"profile-bytes").is_err(),
      "profile bytes helper should report an error when profile.pb cannot be created as a file",
    )
  }
}
