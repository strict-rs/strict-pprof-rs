// Copyright 2019 TiKV Project Authors. Licensed under Apache-2.0.

const PRIME_TABLE_LIMIT: usize = 10_000;

fn is_prime_number_impl(candidate: usize, prime_numbers: &[usize]) -> bool {
  if candidate < PRIME_TABLE_LIMIT {
    return prime_numbers.binary_search(&candidate).is_ok();
  }

  for prime_number in prime_numbers {
    if candidate.is_multiple_of(*prime_number) {
      return false;
    }
  }

  true
}

#[inline(never)]
pub fn is_prime_number(candidate: usize, prime_numbers: &[usize]) -> bool {
  is_prime_number_impl(candidate, prime_numbers)
}

#[inline(never)]
pub fn is_prime_number1(candidate: usize, prime_numbers: &[usize]) -> bool {
  is_prime_number_impl(candidate, prime_numbers)
}

#[inline(always)]
pub fn is_prime_number2(candidate: usize, prime_numbers: &[usize]) -> bool {
  is_prime_number_impl(candidate, prime_numbers)
}

#[inline(never)]
pub fn is_prime_number3(candidate: usize, prime_numbers: &[usize]) -> bool {
  is_prime_number_impl(candidate, prime_numbers)
}

#[inline(never)]
pub fn prepare_prime_numbers() -> Vec<usize> {
  let mut prime_number_table = [true; PRIME_TABLE_LIMIT];
  prime_number_table[0] = false;
  prime_number_table[1] = false;

  for candidate in 2..PRIME_TABLE_LIMIT {
    if !prime_number_table[candidate] {
      continue;
    }

    let mut multiple = candidate * 2;
    while multiple < PRIME_TABLE_LIMIT {
      prime_number_table[multiple] = false;
      multiple += candidate;
    }
  }

  prime_number_table
    .iter()
    .enumerate()
    .skip(2)
    .filter_map(|(candidate, exists)| exists.then_some(candidate))
    .collect()
}

pub fn count_primes(limit: usize, prime_numbers: &[usize]) -> usize {
  let mut prime_count = 0;
  for candidate in 2..limit {
    if is_prime_number(candidate, prime_numbers) {
      prime_count += 1;
    }
  }
  prime_count
}

pub fn count_partitioned_primes(limit: usize, prime_numbers: &[usize]) -> usize {
  let mut prime_count = 0;
  for candidate in 2..limit {
    let is_prime = match candidate % 4 {
      0 => is_prime_number1(candidate, prime_numbers),
      1 => is_prime_number2(candidate, prime_numbers),
      _ => is_prime_number3(candidate, prime_numbers),
    };
    if is_prime {
      prime_count += 1;
    }
  }
  prime_count
}

#[cfg(test)]
mod tests {
  use strict_test_support::TestFailure;
  use strict_test_support::ensure;
  use strict_test_support::ensure_eq;

  use super::*;

  #[test]
  fn prime_table_contains_primes_and_excludes_composites() -> std::result::Result<(), TestFailure> {
    let prime_numbers = prepare_prime_numbers();

    ensure(
      is_prime_number(2, &prime_numbers),
      "prime table should classify the first prime as prime",
    )?;
    ensure(
      is_prime_number(9973, &prime_numbers),
      "prime table should classify primes below the table limit as prime",
    )?;
    ensure(!is_prime_number(1, &prime_numbers), "prime table should not classify one as prime")?;
    ensure(
      !is_prime_number(100, &prime_numbers),
      "prime table should not classify composites below the table limit as prime",
    )
  }

  #[test]
  fn prime_check_uses_divisibility_for_candidates_above_table_limit() -> std::result::Result<(), TestFailure> {
    let prime_numbers = prepare_prime_numbers();

    ensure(
      is_prime_number(PRIME_TABLE_LIMIT + 7, &prime_numbers),
      "candidate above the table limit without a small divisor should be treated as prime by the workload helper",
    )?;
    ensure(
      !is_prime_number(PRIME_TABLE_LIMIT + 8, &prime_numbers),
      "candidate above the table limit with a small divisor should be rejected",
    )
  }

  #[test]
  fn prime_counters_match_for_simple_and_partitioned_workloads() -> std::result::Result<(), TestFailure> {
    let prime_numbers = prepare_prime_numbers();

    ensure_eq(
      &count_primes(30, &prime_numbers),
      &10,
      "prime counter should count primes below the exclusive limit",
    )?;
    ensure_eq(
      &count_partitioned_primes(30, &prime_numbers),
      &10,
      "partitioned prime counter should preserve workload semantics",
    )
  }
}
