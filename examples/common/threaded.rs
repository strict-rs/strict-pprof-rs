// Copyright 2019 TiKV Project Authors. Licensed under Apache-2.0.

use std::sync::Arc;

use crate::prime;
use crate::profile_proto;

const THREAD_WORK_LIMIT: usize = 50_000;

pub fn run_multithread_reports() -> profile_proto::ExampleResult<()> {
  let guard = pprof::ProfilerGuard::new(100)?;
  spawn_prime_workers()?;

  loop {
    if let Ok(report) = guard.report().build() {
      profile_proto::log_report(&report);
    }
    std::thread::sleep(std::time::Duration::from_secs(1));
  }
}

#[cfg(feature = "flamegraph")]
pub fn run_multithread_flamegraphs() -> profile_proto::ExampleResult<()> {
  let guard = pprof::ProfilerGuard::new(100)?;
  spawn_prime_workers()?;

  loop {
    if let Ok(report) = guard.report().build() {
      profile_proto::write_flamegraph(&report)?;
      profile_proto::log_report(&report);
    }
    std::thread::sleep(std::time::Duration::from_secs(1));
  }
}

pub fn run_post_processed_reports() -> profile_proto::ExampleResult<()> {
  let guard = pprof::ProfilerGuard::new(100)?;
  spawn_prime_workers()?;

  loop {
    if let Ok(report) = guard
      .report()
      .frames_post_processor(|frames| {
        frames.thread_name = "PROCESSED".to_owned();
      })
      .build()
    {
      profile_proto::log_report(&report);
    }
    std::thread::sleep(std::time::Duration::from_secs(1));
  }
}

pub fn spawn_prime_workers() -> std::io::Result<()> {
  let prime_numbers = Arc::new(prime::prepare_prime_numbers());

  spawn_named_worker("THREAD_ONE", Arc::clone(&prime_numbers))?;
  spawn_named_worker("THREAD_TWO", Arc::clone(&prime_numbers))?;
  spawn_unnamed_worker(prime_numbers);

  Ok(())
}

fn spawn_named_worker(name: &str, prime_numbers: Arc<Vec<usize>>) -> std::io::Result<()> {
  std::thread::Builder::new()
    .name(name.to_owned())
    .spawn(move || run_worker(prime_numbers))?;
  Ok(())
}

fn spawn_unnamed_worker(prime_numbers: Arc<Vec<usize>>) {
  std::thread::spawn(move || run_worker(prime_numbers));
}

fn run_worker(prime_numbers: Arc<Vec<usize>>) {
  loop {
    let mut prime_count = 0;

    for candidate in 2..THREAD_WORK_LIMIT {
      if prime::is_prime_number(candidate, &prime_numbers) {
        prime_count += 1;
      }
    }

    std::hint::black_box(prime_count);
  }
}
