// Copyright 2019 TiKV Project Authors. Licensed under Apache-2.0.

#[path = "common/prime.rs"]
pub mod prime;
#[path = "common/profile_proto.rs"]
pub mod profile_proto;

fn main() -> pprof::Result<()> {
  let prime_numbers = prime::prepare_prime_numbers();
  let guard = pprof::ProfilerGuard::new(100)?;

  loop {
    let prime_count = prime::count_primes(50_000, &prime_numbers);
    profile_proto::log_prime_count(prime_count);

    if let Ok(report) = guard.report().build() {
      profile_proto::log_report(&report);
    }
  }
}
