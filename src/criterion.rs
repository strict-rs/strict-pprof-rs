// Copyright 2020 TiKV Project Authors. Licensed under Apache-2.0.

use std::fs::File;
#[cfg(any(feature = "prost-codec", feature = "protobuf-codec"))]
use std::io::Write;
use std::marker::PhantomData;
use std::os::raw::c_int;
use std::path::Path;

use criterion::profiler::Profiler;

use crate::ProfilerGuard;
use crate::Report;
#[cfg(feature = "flamegraph")]
use crate::flamegraph::Options as FlamegraphOptions;

pub enum Output<'a> {
  #[cfg(feature = "flamegraph")]
  Flamegraph(Option<Box<FlamegraphOptions>>),

  #[cfg(any(feature = "prost-codec", feature = "protobuf-codec"))]
  Protobuf,

  #[deprecated(note = "This branch is used to include lifetime parameter. Don't use it directly.")]
  _Phantom(PhantomData<&'a ()>),
}

pub struct PProfProfiler<'a, 'b> {
  frequency:       c_int,
  output:          Output<'b>,
  active_profiler: Option<ProfilerGuard<'a>>,
}

impl<'a, 'b> PProfProfiler<'a, 'b> {
  pub fn new(frequency: c_int, output: Output<'b>) -> Self {
    Self {
      frequency,
      output,
      active_profiler: None,
    }
  }

  fn output_filename(&self) -> &'static str {
    match self.output {
      #[cfg(feature = "flamegraph")]
      Output::Flamegraph(_) => "flamegraph.svg",
      #[cfg(any(feature = "prost-codec", feature = "protobuf-codec"))]
      Output::Protobuf => "profile.pb",
      _ => "",
    }
  }

  fn take_report(&mut self) -> Option<Report> {
    let profiler = self.active_profiler.take()?;
    match profiler.report().build() {
      Ok(report) => Some(report),
      Err(err) => {
        log::error!("error while building profile report: {err}");
        None
      }
    }
  }

  fn write_output(&self, report: Report, output_file: File) {
    match &self.output {
      #[cfg(feature = "flamegraph")]
      Output::Flamegraph(options) => write_flamegraph_output(&report, output_file, options.as_deref()),

      #[cfg(any(feature = "prost-codec", feature = "protobuf-codec"))]
      Output::Protobuf => write_protobuf_output(&report, output_file),

      _ => {}
    }
  }
}

#[cfg(feature = "flamegraph")]
fn write_flamegraph_output(report: &Report, output_file: File, options: Option<&FlamegraphOptions>) {
  let default_options = FlamegraphOptions::default();
  let options = options.unwrap_or(&default_options);
  if let Err(err) = report.flamegraph_with_options(output_file, options) {
    log::error!("error while writing flamegraph: {err}");
  }
}

#[cfg(any(feature = "prost-codec", feature = "protobuf-codec"))]
fn write_protobuf_output(report: &Report, mut output_file: File) {
  let profile = match report.pprof() {
    Ok(profile) => profile,
    Err(err) => {
      log::error!("error while building protobuf profile: {err}");
      return;
    }
  };

  let content = match crate::protos::encode_profile(&profile) {
    Ok(content) => content,
    Err(err) => {
      log::error!("error while encoding protobuf profile: {err}");
      return;
    }
  };

  if let Err(err) = output_file.write_all(&content) {
    log::error!("error while writing protobuf profile: {err}");
  }
}

#[cfg(not(any(feature = "prost-codec", feature = "protobuf-codec", feature = "flamegraph")))]
compile_error!("Either feature \"protobuf\" or \"flamegraph\" must be enabled when \"criterion\" feature is enabled.");

impl<'a, 'b> Profiler for PProfProfiler<'a, 'b> {
  fn start_profiling(&mut self, _benchmark_id: &str, _benchmark_dir: &Path) {
    match ProfilerGuard::new(self.frequency) {
      Ok(profiler) => self.active_profiler = Some(profiler),
      Err(err) => log::error!("error while starting profiler: {err}"),
    }
  }

  fn stop_profiling(&mut self, _benchmark_id: &str, benchmark_dir: &Path) {
    // Drain the guard before fallible output setup so path errors cannot leave SIGPROF installed.
    let report = self.take_report();

    if let Err(err) = std::fs::create_dir_all(benchmark_dir) {
      log::error!("error while creating benchmark directory {}: {err}", benchmark_dir.display());
      return;
    }

    let filename = self.output_filename();
    let output_path = benchmark_dir.join(filename);
    let output_file = match File::create(&output_path) {
      Ok(output_file) => output_file,
      Err(err) => {
        log::error!("error while creating {}: {err}", output_path.display());
        return;
      }
    };

    if let Some(report) = report {
      self.write_output(report, output_file);
    }
  }
}

#[cfg(test)]
mod tests {
  use strict_test_support::TestFailure;
  use strict_test_support::ensure;
  #[cfg(feature = "flamegraph")]
  use strict_test_support::ensure_contains;
  use strict_test_support::ensure_ok;

  use super::*;

  #[cfg(feature = "flamegraph")]
  fn read_flamegraph_output(benchmark_dir: &std::path::Path) -> std::result::Result<String, TestFailure> {
    ensure_ok(
      std::fs::read_to_string(benchmark_dir.join("flamegraph.svg")),
      "flamegraph output should be readable",
    )
  }

  fn ensure_stop_without_active_creates_empty_output_file(output: Output<'static>, filename: &str) -> std::result::Result<(), TestFailure> {
    let mut profiler = PProfProfiler::new(100, output);
    let dir = ensure_ok(tempfile::tempdir(), "tempdir should be created")?;

    criterion::profiler::Profiler::stop_profiling(&mut profiler, "bench", dir.path());

    let output_path = dir.path().join(filename);
    ensure(output_path.is_file(), "stop should create the configured output file")?;
    let contents = ensure_ok(std::fs::read(output_path), "output file should be readable")?;
    ensure(contents.is_empty(), "without an active profiler there should be no report output")
  }

  fn ensure_global_profiler_can_restart_after_failed_criterion_stop() -> std::result::Result<(), TestFailure> {
    let restarted_profiler = ensure_ok(
      crate::ProfilerGuard::new(1),
      "global profiler should restart after failed criterion stop",
    )?;
    drop(restarted_profiler);
    Ok(())
  }

  fn ensure_stop_clears_active_profiler_when_output_file_cannot_be_created(
    output: Output<'static>,
    filename: &str,
  ) -> std::result::Result<(), TestFailure> {
    let _guard = crate::profiler::PROFILER_SIGNAL_TEST_LOCK.lock();
    let mut profiler = PProfProfiler::new(1, output);
    let dir = ensure_ok(tempfile::tempdir(), "tempdir should be created")?;
    let output_path = dir.path().join(filename);
    ensure_ok(std::fs::create_dir(&output_path), "directory at output path should be created")?;

    criterion::profiler::Profiler::start_profiling(&mut profiler, "bench", dir.path());
    ensure(
      profiler.active_profiler.is_some(),
      "criterion profiler should start before output file creation failure",
    )?;
    criterion::profiler::Profiler::stop_profiling(&mut profiler, "bench", dir.path());

    ensure(
      profiler.active_profiler.is_none(),
      "failed output creation should consume the active profiler guard",
    )?;
    ensure(output_path.is_dir(), "blocked output path should remain a directory")?;
    ensure_global_profiler_can_restart_after_failed_criterion_stop()
  }

  fn ensure_stop_clears_active_profiler_when_benchmark_directory_cannot_be_created(
    output: Output<'static>,
  ) -> std::result::Result<(), TestFailure> {
    let _guard = crate::profiler::PROFILER_SIGNAL_TEST_LOCK.lock();
    let mut profiler = PProfProfiler::new(1, output);
    let benchmark_dir = ensure_ok(tempfile::NamedTempFile::new(), "temp benchmark file should be created")?;
    let benchmark_path = benchmark_dir.path().to_owned();

    criterion::profiler::Profiler::start_profiling(&mut profiler, "bench", &benchmark_path);
    ensure(
      profiler.active_profiler.is_some(),
      "criterion profiler should start before benchmark directory creation failure",
    )?;
    criterion::profiler::Profiler::stop_profiling(&mut profiler, "bench", &benchmark_path);

    ensure(
      profiler.active_profiler.is_none(),
      "failed benchmark directory creation should consume the active profiler guard",
    )?;
    ensure(
      benchmark_path.is_file(),
      "blocked benchmark directory path should remain an existing file",
    )?;
    ensure_global_profiler_can_restart_after_failed_criterion_stop()
  }

  #[cfg(any(feature = "prost-codec", feature = "protobuf-codec"))]
  #[test]
  fn protobuf_output_uses_profile_filename_and_writes_bytes() -> std::result::Result<(), TestFailure> {
    let _guard = crate::profiler::PROFILER_SIGNAL_TEST_LOCK.lock();
    let mut profiler = PProfProfiler::new(1, Output::Protobuf);
    let dir = ensure_ok(tempfile::tempdir(), "tempdir should be created")?;

    ensure(
      profiler.output_filename() == "profile.pb",
      "protobuf output should use profile filename",
    )?;
    criterion::profiler::Profiler::start_profiling(&mut profiler, "bench", dir.path());
    criterion::profiler::Profiler::stop_profiling(&mut profiler, "bench", dir.path());

    let bytes = ensure_ok(std::fs::read(dir.path().join("profile.pb")), "profile bytes should be readable")?;
    ensure(!bytes.is_empty(), "protobuf output should write encoded profile bytes")
  }

  #[cfg(any(feature = "prost-codec", feature = "protobuf-codec"))]
  #[test]
  fn new_protobuf_profiler_starts_inactive_and_uses_profile_filename() -> std::result::Result<(), TestFailure> {
    let profiler = PProfProfiler::new(100, Output::Protobuf);

    ensure(
      profiler.active_profiler.is_none(),
      "new protobuf criterion profiler should not start sampling until Criterion calls start",
    )?;
    ensure(
      profiler.output_filename() == "profile.pb",
      "protobuf output should use profile filename",
    )
  }

  #[cfg(any(feature = "prost-codec", feature = "protobuf-codec"))]
  #[test]
  fn stop_without_active_protobuf_profiler_creates_output_file_only() -> std::result::Result<(), TestFailure> {
    ensure_stop_without_active_creates_empty_output_file(Output::Protobuf, "profile.pb")
  }

  #[cfg(any(feature = "prost-codec", feature = "protobuf-codec"))]
  #[test]
  fn stop_clears_active_protobuf_profiler_when_output_file_cannot_be_created() -> std::result::Result<(), TestFailure> {
    ensure_stop_clears_active_profiler_when_output_file_cannot_be_created(Output::Protobuf, "profile.pb")
  }

  #[cfg(any(feature = "prost-codec", feature = "protobuf-codec"))]
  #[test]
  fn stop_clears_active_protobuf_profiler_when_benchmark_directory_cannot_be_created() -> std::result::Result<(), TestFailure> {
    ensure_stop_clears_active_profiler_when_benchmark_directory_cannot_be_created(Output::Protobuf)
  }

  #[cfg(feature = "flamegraph")]
  #[test]
  fn new_flamegraph_profiler_starts_inactive_and_uses_svg_filename() -> std::result::Result<(), TestFailure> {
    let profiler = PProfProfiler::new(100, Output::Flamegraph(None));

    ensure(
      profiler.active_profiler.is_none(),
      "new criterion profiler should not start sampling until Criterion calls start",
    )?;
    ensure(
      profiler.output_filename() == "flamegraph.svg",
      "flamegraph output should use the svg filename",
    )
  }

  #[cfg(feature = "flamegraph")]
  #[test]
  fn take_report_without_active_profiler_returns_none() -> std::result::Result<(), TestFailure> {
    let mut profiler = PProfProfiler::new(100, Output::Flamegraph(None));

    ensure(
      profiler.take_report().is_none(),
      "criterion profiler without an active guard should not build a report",
    )
  }

  #[cfg(feature = "flamegraph")]
  #[test]
  fn stop_without_active_profiler_creates_output_file_only() -> std::result::Result<(), TestFailure> {
    ensure_stop_without_active_creates_empty_output_file(Output::Flamegraph(None), "flamegraph.svg")
  }

  #[cfg(feature = "flamegraph")]
  #[test]
  fn stop_with_invalid_benchmark_directory_returns_without_output() -> std::result::Result<(), TestFailure> {
    let mut profiler = PProfProfiler::new(100, Output::Flamegraph(None));
    let benchmark_dir = ensure_ok(tempfile::NamedTempFile::new(), "temp benchmark file should be created")?;
    let benchmark_path = benchmark_dir.path().to_owned();

    criterion::profiler::Profiler::stop_profiling(&mut profiler, "bench", &benchmark_path);

    ensure(
      benchmark_path.is_file(),
      "invalid benchmark directory should remain an existing file",
    )
  }

  #[cfg(feature = "flamegraph")]
  #[test]
  fn start_and_stop_flamegraph_profiler_writes_report() -> std::result::Result<(), TestFailure> {
    let _guard = crate::profiler::PROFILER_SIGNAL_TEST_LOCK.lock();
    let mut profiler = PProfProfiler::new(1, Output::Flamegraph(None));
    let dir = ensure_ok(tempfile::tempdir(), "tempdir should be created")?;

    criterion::profiler::Profiler::start_profiling(&mut profiler, "bench", dir.path());

    ensure(
      profiler.active_profiler.is_some(),
      "starting criterion profiler should install an active pprof guard",
    )?;

    criterion::profiler::Profiler::stop_profiling(&mut profiler, "bench", dir.path());

    ensure(
      profiler.active_profiler.is_none(),
      "stopping criterion profiler should consume the active guard",
    )?;
    let contents = read_flamegraph_output(dir.path())?;
    ensure_contains(&contents, "<svg", "active criterion profiler should write svg output")
  }

  #[cfg(feature = "flamegraph")]
  #[test]
  fn start_leaves_profiler_inactive_when_global_profiler_is_running() -> std::result::Result<(), TestFailure> {
    let _guard = crate::profiler::PROFILER_SIGNAL_TEST_LOCK.lock();
    let mut first_profiler = PProfProfiler::new(1, Output::Flamegraph(None));
    let mut second_profiler = PProfProfiler::new(1, Output::Flamegraph(None));
    let dir = ensure_ok(tempfile::tempdir(), "tempdir should be created")?;

    criterion::profiler::Profiler::start_profiling(&mut first_profiler, "first", dir.path());
    criterion::profiler::Profiler::start_profiling(&mut second_profiler, "second", dir.path());

    ensure(
      first_profiler.active_profiler.is_some(),
      "first criterion profiler should own the active pprof guard",
    )?;
    ensure(
      second_profiler.active_profiler.is_none(),
      "second criterion profiler should remain inactive while pprof is already running",
    )?;

    criterion::profiler::Profiler::stop_profiling(&mut first_profiler, "first", dir.path());
    ensure(
      first_profiler.active_profiler.is_none(),
      "first criterion profiler should stop cleanly after contention test",
    )
  }

  #[cfg(feature = "flamegraph")]
  #[test]
  fn stop_clears_active_flamegraph_profiler_when_output_file_cannot_be_created() -> std::result::Result<(), TestFailure> {
    ensure_stop_clears_active_profiler_when_output_file_cannot_be_created(Output::Flamegraph(None), "flamegraph.svg")
  }

  #[cfg(feature = "flamegraph")]
  #[test]
  fn stop_clears_active_flamegraph_profiler_when_benchmark_directory_cannot_be_created() -> std::result::Result<(), TestFailure> {
    ensure_stop_clears_active_profiler_when_benchmark_directory_cannot_be_created(Output::Flamegraph(None))
  }

  #[cfg(feature = "flamegraph")]
  #[test]
  fn stop_with_invalid_flamegraph_options_consumes_profiler_without_svg_output() -> std::result::Result<(), TestFailure> {
    let _guard = crate::profiler::PROFILER_SIGNAL_TEST_LOCK.lock();
    let options = FlamegraphOptions {
      frame_height: 0,
      ..FlamegraphOptions::default()
    };
    let mut profiler = PProfProfiler::new(1, Output::Flamegraph(Some(Box::new(options))));
    let dir = ensure_ok(tempfile::tempdir(), "tempdir should be created")?;

    ensure(
      profiler.output_filename() == "flamegraph.svg",
      "flamegraph output should use svg filename",
    )?;
    criterion::profiler::Profiler::start_profiling(&mut profiler, "bench", dir.path());
    criterion::profiler::Profiler::stop_profiling(&mut profiler, "bench", dir.path());

    ensure(
      profiler.active_profiler.is_none(),
      "invalid flamegraph options should still consume the completed profiler",
    )?;
    let contents = read_flamegraph_output(dir.path())?;
    ensure(
      contents.is_empty(),
      "invalid flamegraph options should leave the output file empty instead of writing partial svg",
    )
  }
}
