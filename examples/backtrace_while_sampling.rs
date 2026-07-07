// Copyright 2021 TiKV Project Authors. Licensed under Apache-2.0.

#[path = "common/prime.rs"]
pub mod prime;
#[path = "common/profile_proto.rs"]
pub mod profile_proto;

fn deep_recursive(depth: i32) {
  if depth > 0 {
    deep_recursive(depth - 1);
  } else {
    backtrace::Backtrace::new();
  }
}

fn main() -> profile_proto::ExampleResult<()> {
  let guard = pprof::ProfilerGuardBuilder::default()
    .frequency(1000)
    .blocklist(&["libc", "libgcc", "pthread"])
    .build()?;

  for _ in 0..10000 {
    deep_recursive(20);
  }

  if let Ok(report) = guard.report().build() {
    profile_proto::write_flamegraph(&report)?;
    profile_proto::log_report(&report);
  };

  Ok(())
}
