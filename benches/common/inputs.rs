// Copyright 2019 TiKV Project Authors. Licensed under Apache-2.0.

pub const SAMPLE_SIZE: usize = 1000;

pub fn random_u64_values() -> Vec<u64> {
  let mut samples = Vec::with_capacity(SAMPLE_SIZE);
  for _ in 0..samples.capacity() {
    samples.push(rand::random());
  }
  samples
}
