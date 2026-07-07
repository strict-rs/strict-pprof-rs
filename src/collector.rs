// Copyright 2019 TiKV Project Authors. Licensed under Apache-2.0.

use std::collections::hash_map::DefaultHasher;
use std::convert::TryInto;
use std::fmt::Debug;
use std::hash::Hash;
use std::hash::Hasher;
use std::io::Read;
use std::io::Seek;
use std::io::SeekFrom;
use std::io::Write;

use aligned_vec::AVec;
use tempfile::NamedTempFile;

use crate::frames::UnresolvedFrames;

pub const BUCKETS: usize = 1 << 12;
pub const BUCKETS_ASSOCIATIVITY: usize = 4;
pub const BUFFER_LENGTH: usize = (1 << 18) / std::mem::size_of::<Entry<UnresolvedFrames>>();

#[derive(Debug)]
pub struct Entry<T> {
  pub item:  T,
  pub count: isize,
}

impl<T: Default> Default for Entry<T> {
  fn default() -> Self {
    Entry {
      item:  Default::default(),
      count: 0,
    }
  }
}

#[derive(Debug)]
pub struct Bucket<T: 'static> {
  pub length: usize,
  entries:    Box<[Entry<T>; BUCKETS_ASSOCIATIVITY]>,
}

impl<T: Eq + Default> Default for Bucket<T> {
  fn default() -> Bucket<T> {
    let entries = Box::default();

    Self {
      length: 0,
      entries,
    }
  }
}

impl<T: Eq> Bucket<T> {
  pub fn add(&mut self, key: T, count: isize) -> Option<Entry<T>> {
    let mut done = false;
    self.entries[0..self.length].iter_mut().for_each(|ele| {
      if ele.item == key {
        ele.count += count;
        done = true;
      }
    });

    if done {
      None
    } else if self.length < BUCKETS_ASSOCIATIVITY {
      let ele = &mut self.entries[self.length];
      ele.item = key;
      ele.count = count;

      self.length += 1;
      None
    } else {
      let min_index = self.min_count_entry_index();
      let mut new_entry = Entry {
        item: key,
        count,
      };
      std::mem::swap(&mut self.entries[min_index], &mut new_entry);
      Some(new_entry)
    }
  }

  fn min_count_entry_index(&self) -> usize {
    let mut min_index = 0;
    let mut min_count = self.entries[0].count;
    for index in 1..self.length {
      let count = self.entries[index].count;
      if count >= min_count {
        continue;
      }
      min_index = index;
      min_count = count;
    }
    min_index
  }

  pub fn iter(&self) -> BucketIterator<'_, T> {
    BucketIterator::<T> {
      related_bucket: self,
      index:          0,
    }
  }
}

pub struct BucketIterator<'a, T: 'static> {
  related_bucket: &'a Bucket<T>,
  index:          usize,
}

impl<'a, T> Iterator for BucketIterator<'a, T> {
  type Item = &'a Entry<T>;

  fn next(&mut self) -> Option<Self::Item> {
    if self.index < self.related_bucket.length {
      self.index += 1;
      Some(&self.related_bucket.entries[self.index - 1])
    } else {
      None
    }
  }
}

pub struct HashCounter<T: Hash + Eq + 'static> {
  buckets: Box<[Bucket<T>; BUCKETS]>,
}

impl<T: Hash + Eq + Default + Debug> Default for HashCounter<T> {
  fn default() -> Self {
    let mut v: Vec<Bucket<T>> = Vec::with_capacity(BUCKETS);
    v.resize_with(BUCKETS, Default::default);
    let buckets = v.into_boxed_slice().try_into().unwrap();

    Self {
      buckets,
    }
  }
}

impl<T: Hash + Eq> HashCounter<T> {
  fn hash(key: &T) -> u64 {
    let mut s = DefaultHasher::new();
    key.hash(&mut s);
    s.finish()
  }

  pub fn add(&mut self, key: T, count: isize) -> Option<Entry<T>> {
    let hash_value = Self::hash(&key);
    let bucket = &mut self.buckets[(hash_value % BUCKETS as u64) as usize];

    bucket.add(key, count)
  }

  pub fn iter(&self) -> impl Iterator<Item = &Entry<T>> {
    let mut iter: Box<dyn Iterator<Item = &Entry<T>>> = Box::new(self.buckets[0].iter().chain(std::iter::empty()));
    for bucket in self.buckets[1..].iter() {
      iter = Box::new(iter.chain(bucket.iter()));
    }

    iter
  }
}

pub struct TempFdArray<T: 'static> {
  file:         NamedTempFile,
  buffer:       Box<[T; BUFFER_LENGTH]>,
  buffer_index: usize,
  flush_n:      usize,
}

impl<T: Default + Debug> TempFdArray<T> {
  fn new() -> std::io::Result<TempFdArray<T>> {
    let file = NamedTempFile::new()?;

    let mut v: Vec<T> = Vec::with_capacity(BUFFER_LENGTH);
    v.resize_with(BUFFER_LENGTH, Default::default);
    let buffer = v.into_boxed_slice().try_into().unwrap();

    Ok(Self {
      file,
      buffer,
      buffer_index: 0,
      flush_n: 0,
    })
  }
}

impl<T> TempFdArray<T> {
  fn flush_buffer(&mut self) -> std::io::Result<()> {
    self.buffer_index = 0;
    let buf = unsafe { std::slice::from_raw_parts(self.buffer.as_ptr() as *const u8, BUFFER_LENGTH * std::mem::size_of::<T>()) };
    self.flush_n += 1;
    self.file.write_all(buf)?;

    Ok(())
  }

  fn push(&mut self, entry: T) -> std::io::Result<()> {
    if self.buffer_index >= BUFFER_LENGTH {
      self.flush_buffer()?;
    }

    self.buffer[self.buffer_index] = entry;
    self.buffer_index += 1;

    Ok(())
  }

  fn try_iter(&self) -> std::io::Result<impl Iterator<Item = &T>> {
    let size = BUFFER_LENGTH * self.flush_n * std::mem::size_of::<T>();

    let mut file_vec = AVec::with_capacity(std::mem::align_of::<T>(), size);
    let mut file = self.file.reopen()?;

    unsafe {
      // it's safe as the capacity is initialized to `size`, and it'll be filled with `size` bytes
      file_vec.set_len(size);
    }
    file.read_exact(&mut file_vec[0..size])?;
    file.seek(SeekFrom::End(0))?;

    Ok(TempFdArrayIterator {
      buffer: &self.buffer[0..self.buffer_index],
      file_vec,
      index: 0,
    })
  }
}

pub struct TempFdArrayIterator<'a, T> {
  pub buffer:   &'a [T],
  pub file_vec: AVec<u8>,
  pub index:    usize,
}

impl<'a, T> Iterator for TempFdArrayIterator<'a, T> {
  type Item = &'a T;

  fn next(&mut self) -> Option<Self::Item> {
    if self.index < self.buffer.len() {
      self.index += 1;
      Some(&self.buffer[self.index - 1])
    } else {
      let length = self.file_vec.len() / std::mem::size_of::<T>();
      let ts = unsafe { std::slice::from_raw_parts(self.file_vec.as_ptr() as *const T, length) };
      if self.index - self.buffer.len() < ts.len() {
        self.index += 1;
        Some(&ts[self.index - self.buffer.len() - 1])
      } else {
        None
      }
    }
  }
}

pub struct Collector<T: Hash + Eq + 'static> {
  map:        HashCounter<T>,
  temp_array: TempFdArray<Entry<T>>,
}

impl<T: Hash + Eq + Default + Debug + 'static> Collector<T> {
  pub fn new() -> std::io::Result<Self> {
    Ok(Self {
      map:        HashCounter::<T>::default(),
      temp_array: TempFdArray::<Entry<T>>::new()?,
    })
  }
}

impl<T: Hash + Eq + 'static> Collector<T> {
  pub fn add(&mut self, key: T, count: isize) -> std::io::Result<()> {
    if let Some(evict) = self.map.add(key, count) {
      self.temp_array.push(evict)?;
    }

    Ok(())
  }

  pub fn try_iter(&self) -> std::io::Result<impl Iterator<Item = &Entry<T>>> {
    Ok(self.map.iter().chain(self.temp_array.try_iter()?))
  }
}

#[cfg(test)]
mod test_utils {
  use std::collections::BTreeMap;

  use super::*;

  pub fn add_map<T: std::cmp::Ord + Copy>(hashmap: &mut BTreeMap<T, isize>, entry: &Entry<T>) {
    match hashmap.get_mut(&entry.item) {
      None => {
        hashmap.insert(entry.item, entry.count);
      }
      Some(count) => *count += entry.count,
    }
  }
}

#[cfg(test)]
mod tests {
  use std::collections::BTreeMap;

  use strict_test_support::TestFailure;
  use strict_test_support::ensure;
  use strict_test_support::ensure_eq;
  use strict_test_support::ensure_ok;

  use super::*;

  const SAMPLE_GROUPS: usize = (1 << 12) * 4;

  fn bucket_entries(bucket: &Bucket<usize>) -> Vec<(usize, isize)> {
    bucket.iter().map(|entry| (entry.item, entry.count)).collect::<Vec<_>>()
  }

  fn sample_count(sample_index: usize) -> isize {
    (sample_index % 4) as isize
  }

  fn record_eviction<T: std::cmp::Ord + Copy>(real_map: &mut BTreeMap<T, isize>, evicted_entry: Option<Entry<T>>) {
    if let Some(evicted_entry) = evicted_entry {
      test_utils::add_map(real_map, &evicted_entry);
    }
  }

  fn ensure_reconstructed_counts<T, F>(
    real_map: &BTreeMap<T, isize>,
    sample_groups: usize,
    make_key: F,
  ) -> std::result::Result<(), TestFailure>
  where
    T: Ord,
    F: Fn(usize) -> T,
  {
    for sample_index in 0..sample_groups {
      let expected_count = sample_count(sample_index);
      let actual_count = real_map.get(&make_key(sample_index)).copied().unwrap_or_default();
      ensure_eq(
        &actual_count,
        &expected_count,
        "reconstructed count should match expected sample count",
      )?;
    }
    Ok(())
  }

  #[test]
  fn bucket_updates_existing_key_without_eviction_or_length_growth() -> std::result::Result<(), TestFailure> {
    let mut bucket = Bucket::<usize>::default();

    ensure(bucket.add(7, 2).is_none(), "first bucket insertion should not evict an entry")?;
    ensure(
      bucket.add(7, 3).is_none(),
      "updating an existing bucket key should not evict an entry",
    )?;

    ensure_eq(&bucket.length, &1, "updating an existing key should not grow bucket length")?;
    ensure(
      bucket_entries(&bucket) == vec![(7, 5)],
      "existing bucket key should accumulate counts in place",
    )
  }

  #[test]
  fn bucket_eviction_spills_lowest_count_entry_and_keeps_new_entry() -> std::result::Result<(), TestFailure> {
    let mut bucket = Bucket::<usize>::default();

    for (key, count) in [(10, 5), (20, 3), (30, 4), (40, 6)] {
      ensure(
        bucket.add(key, count).is_none(),
        "filling bucket below associativity should not evict",
      )?;
    }

    let evicted = bucket.add(50, 7);
    match evicted {
      Some(entry) => {
        ensure_eq(&entry.item, &20, "bucket should evict the lowest-count entry")?;
        ensure_eq(&entry.count, &3, "evicted entry should preserve its accumulated count")?;
      }
      None => {
        return ensure(false, "full bucket insertion should evict one entry");
      }
    }

    let entries = bucket_entries(&bucket);
    ensure_eq(&bucket.length, &BUCKETS_ASSOCIATIVITY, "full bucket should remain full")?;
    ensure(
      entries.iter().any(|(key, count)| (*key, *count) == (50, 7)),
      "full bucket should retain the newly inserted entry",
    )?;
    ensure(
      !entries.iter().any(|(key, _count)| *key == 20),
      "evicted key should not remain in the bucket",
    )
  }

  #[test]
  fn bucket_iterator_yields_only_live_entries() -> std::result::Result<(), TestFailure> {
    let mut bucket = Bucket::<usize>::default();

    ensure(
      bucket_entries(&bucket).is_empty(),
      "empty bucket iterator should not expose default backing slots",
    )?;
    ensure(bucket.add(11, 1).is_none(), "single bucket insertion should not evict an entry")?;

    ensure(
      bucket_entries(&bucket) == vec![(11, 1)],
      "bucket iterator should expose only inserted entries",
    )
  }

  #[test]
  fn temp_fd_array_iterates_buffered_entries_without_flush() -> std::result::Result<(), TestFailure> {
    let mut entries = ensure_ok(TempFdArray::<usize>::new(), "temp fd array should be created")?;

    ensure_ok(entries.push(7), "first buffered entry should be pushed")?;
    ensure_ok(entries.push(11), "second buffered entry should be pushed")?;

    let collected = ensure_ok(entries.try_iter(), "buffered entries should iterate")?
      .copied()
      .collect::<Vec<_>>();
    ensure(
      collected == vec![7, 11],
      "unflushed temp fd array should expose buffered entries in insertion order",
    )
  }

  #[test]
  fn temp_fd_array_iterates_current_buffer_and_flushed_entries() -> std::result::Result<(), TestFailure> {
    ensure(BUFFER_LENGTH > 0, "collector buffer length should be nonzero")?;
    let total_entries = BUFFER_LENGTH.saturating_add(2);
    let mut entries = ensure_ok(TempFdArray::<usize>::new(), "temp fd array should be created")?;

    for entry in 0..total_entries {
      ensure_ok(entries.push(entry), "temp fd array entry should be pushed")?;
    }

    let collected = ensure_ok(entries.try_iter(), "spilled entries should iterate")?
      .copied()
      .collect::<Vec<_>>();
    ensure_eq(
      &collected.len(),
      &total_entries,
      "temp fd array iterator should include buffered and flushed entries",
    )?;
    ensure(
      collected.first().copied() == Some(BUFFER_LENGTH),
      "iterator should expose the current in-memory buffer before flushed storage",
    )?;
    ensure(
      collected.get(1).copied() == Some(BUFFER_LENGTH.saturating_add(1)),
      "iterator should include all current buffered entries",
    )?;
    ensure(
      collected.contains(&0),
      "iterator should include entries previously flushed to the backing file",
    )?;
    ensure(
      collected.contains(&BUFFER_LENGTH.saturating_sub(1)),
      "iterator should include the last entry from the flushed buffer",
    )
  }

  #[test]
  fn stack_hash_counter() -> std::result::Result<(), TestFailure> {
    let mut stack_hash_counter = HashCounter::<usize>::default();
    stack_hash_counter.add(0, 1);
    stack_hash_counter.add(1, 1);
    stack_hash_counter.add(1, 1);

    let mut real_map = BTreeMap::new();
    for entry in stack_hash_counter.iter() {
      test_utils::add_map(&mut real_map, entry);
    }

    ensure_eq(
      &real_map.get(&0).copied().unwrap_or_default(),
      &1,
      "first key should have one sample",
    )?;
    ensure_eq(
      &real_map.get(&1).copied().unwrap_or_default(),
      &2,
      "second key should merge two samples",
    )?;
    ensure_eq(&real_map.len(), &2, "hash counter should contain only inserted keys")
  }

  #[test]
  fn evict_test() -> std::result::Result<(), TestFailure> {
    let mut stack_hash_counter = HashCounter::<usize>::default();
    let mut real_map = BTreeMap::new();

    for sample_index in 0..(1 << 10) * 4 {
      for _ in 0..(sample_index % 4) {
        record_eviction(&mut real_map, stack_hash_counter.add(sample_index, 1));
      }
    }

    for entry in stack_hash_counter.iter() {
      test_utils::add_map(&mut real_map, entry);
    }

    ensure_reconstructed_counts(&real_map, (1 << 10) * 4, |sample_index| sample_index)
  }

  #[test]
  fn collector_test() -> std::result::Result<(), TestFailure> {
    let mut collector = ensure_ok(Collector::new(), "collector should be created")?;
    let mut real_map = BTreeMap::new();

    for sample_index in 0..SAMPLE_GROUPS {
      for _ in 0..(sample_index % 4) {
        ensure_ok(collector.add(sample_index, 1), "sample should be added")?;
      }
    }

    for entry in ensure_ok(collector.try_iter(), "collector entries should iterate")? {
      test_utils::add_map(&mut real_map, entry);
    }

    ensure_reconstructed_counts(&real_map, SAMPLE_GROUPS, |sample_index| sample_index)
  }

  #[derive(Debug, Hash, Eq, PartialEq, PartialOrd, Ord, Default, Clone, Copy)]
  struct AlignTest {
    a: u16,
    b: u32,
    c: u64,
    d: u64,
  }

  // collector_align_test uses a bigger item to test the alignment of the collector
  #[test]
  fn collector_align_test() -> std::result::Result<(), TestFailure> {
    let mut collector = ensure_ok(Collector::new(), "collector should be created")?;
    let mut real_map = BTreeMap::new();

    for sample_index in 0..SAMPLE_GROUPS {
      for _ in 0..(sample_index % 4) {
        ensure_ok(collector.add(align_test(sample_index), 1), "aligned sample should be added")?;
      }
    }

    for entry in ensure_ok(collector.try_iter(), "collector entries should iterate")? {
      test_utils::add_map(&mut real_map, entry);
    }

    ensure_reconstructed_counts(&real_map, SAMPLE_GROUPS, align_test)
  }

  fn align_test(sample_index: usize) -> AlignTest {
    AlignTest {
      a: sample_index as u16,
      b: sample_index as u32,
      c: sample_index as u64,
      d: sample_index as u64,
    }
  }
}
