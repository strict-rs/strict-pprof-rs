use std::fs::File;
use std::io::BufRead;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::UNIX_EPOCH;

use arc_swap::ArcSwap;
use once_cell::sync::Lazy;

use crate::Error;
use crate::Symbol;

#[derive(Debug)]
pub struct PerfMap {
  ranges: Vec<(usize, usize, String)>,
}

impl PerfMap {
  pub fn new(file: File) -> Option<Self> {
    let reader = std::io::BufReader::new(file);
    let mut ranges = Vec::new();
    for line in reader.lines() {
      let line = line.ok()?;
      // The format of perf map is:
      // <start addr> <len addr> <name>
      // where <start addr> and <len addr> are hexadecimal numbers.
      // where <name> may contain spaces.
      let mut parts = line.split_whitespace();
      let start = usize::from_str_radix(parts.next()?, 16).ok()?;
      let len = usize::from_str_radix(parts.next()?, 16).ok()?;
      let end = start.checked_add(len)?;
      let name = parts.collect::<Vec<_>>().join(" ");
      ranges.push((start, end, name));
    }
    Some(Self {
      ranges,
    })
  }

  pub fn find(&self, addr: usize) -> Option<PerfMapSymbol> {
    for (start, end, name) in &self.ranges {
      if *start <= addr && addr < *end {
        return Some(PerfMapSymbol(name.clone()));
      }
    }
    None
  }
}

#[derive(Debug)]
pub struct PerfMapSymbol(String);

impl From<PerfMapSymbol> for Symbol {
  fn from(symbol: PerfMapSymbol) -> Self {
    Symbol {
      name:     Some(symbol.0.into_bytes()),
      addr:     None,
      filename: None,
      lineno:   None,
    }
  }
}

fn ensure_perf_map_file(path: &Path) -> Result<(), Error> {
  std::fs::OpenOptions::new()
    .create(true)
    .truncate(false)
    .write(true)
    .open(path)
    .map_err(Error::PerfMapFile)?;
  Ok(())
}

static LAST_LOADED_MTIME_SECONDS: AtomicU64 = AtomicU64::new(0);

fn process_perf_map_path() -> PathBuf {
  PathBuf::from("/tmp/").join(format!("perf-{}.map", std::process::id()))
}

fn init_resolver_at(path: &Path, last_loaded_mtime_seconds: &AtomicU64) -> Option<PerfMap> {
  let file = File::open(path).ok()?;
  let modified_time = path.metadata().ok()?.modified().ok()?;
  let modified_seconds = modified_time.duration_since(UNIX_EPOCH).ok()?.as_secs();
  if last_loaded_mtime_seconds.load(Ordering::Relaxed) == modified_seconds {
    return None;
  }

  let perf_map = PerfMap::new(file)?;
  last_loaded_mtime_seconds.store(modified_seconds, Ordering::Relaxed);
  Some(perf_map)
}

fn init_resolver() -> Option<PerfMap> {
  init_resolver_at(&process_perf_map_path(), &LAST_LOADED_MTIME_SECONDS)
}

static RESOLVER: Lazy<ArcSwap<Option<PerfMap>>> = Lazy::new(|| {
  // this makes sure the file exists
  ensure_perf_map_file(&process_perf_map_path()).ok();
  ArcSwap::from(Arc::new(init_resolver()))
});

fn refresh_process_resolver() {
  let perf_map = init_resolver();

  if perf_map.is_none() {
    return;
  }

  RESOLVER.store(Arc::new(perf_map));
}

pub fn get_resolver() -> Arc<Option<PerfMap>> {
  std::thread::spawn(refresh_process_resolver);

  RESOLVER.load().clone()
}

#[cfg(test)]
mod tests {
  use parking_lot::Mutex;
  use strict_test_support::TestFailure;
  use strict_test_support::ensure;
  use strict_test_support::ensure_contains;
  use strict_test_support::ensure_eq;
  use strict_test_support::ensure_ok;
  use strict_test_support::ensure_some;

  use super::*;

  static PROCESS_PERF_MAP_LOCK: Mutex<()> = Mutex::new(());

  struct ProcessPerfMapGuard {
    path: PathBuf,
    original: Option<Vec<u8>>,
    previous_loaded_mtime_seconds: u64,
  }

  impl ProcessPerfMapGuard {
    fn capture() -> std::result::Result<Self, TestFailure> {
      let path = process_perf_map_path();
      let original = ensure_ok(
        read_optional_file(&path),
        "process perf-map contents should be readable when present",
      )?;
      let previous_loaded_mtime_seconds = LAST_LOADED_MTIME_SECONDS.load(Ordering::Relaxed);

      Ok(Self {
        path,
        original,
        previous_loaded_mtime_seconds,
      })
    }
  }

  impl Drop for ProcessPerfMapGuard {
    fn drop(&mut self) {
      match &self.original {
        Some(contents) => {
          let _ = std::fs::write(&self.path, contents);
        }
        None => {
          let _ = std::fs::remove_file(&self.path);
        }
      }
      LAST_LOADED_MTIME_SECONDS.store(self.previous_loaded_mtime_seconds, Ordering::Relaxed);
    }
  }

  fn read_optional_file(path: &Path) -> std::io::Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
      Ok(contents) => Ok(Some(contents)),
      Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
      Err(err) => Err(err),
    }
  }

  fn temp_perf_map_file(contents: &str) -> std::result::Result<File, TestFailure> {
    let mut file = ensure_ok(tempfile::tempfile(), "temp perf map should be created")?;
    ensure_ok(
      std::io::Write::write_all(&mut file, contents.as_bytes()),
      "perf map contents should be written",
    )?;
    ensure_ok(std::io::Seek::rewind(&mut file), "perf map file should rewind")?;
    Ok(file)
  }

  fn perf_map_from(contents: &str) -> std::result::Result<PerfMap, TestFailure> {
    let file = temp_perf_map_file(contents)?;
    ensure_some(PerfMap::new(file), "perf map should parse")
  }

  #[test]
  fn read_optional_file_distinguishes_missing_files_from_other_read_errors() -> std::result::Result<(), TestFailure> {
    let dir = ensure_ok(tempfile::tempdir(), "tempdir should be created")?;
    let missing_path = dir.path().join("missing-perf.map");
    let directory_path = dir.path().join("perf-map-directory");
    ensure_ok(std::fs::create_dir(&directory_path), "directory path should be created")?;

    ensure(
      ensure_ok(read_optional_file(&missing_path), "missing file should be read as optional")?.is_none(),
      "missing perf-map files should be represented as None",
    )?;
    ensure(
      read_optional_file(&directory_path).is_err(),
      "non-file read failures should be preserved as errors",
    )
  }

  #[test]
  fn ensure_perf_map_file_creates_missing_file() -> std::result::Result<(), TestFailure> {
    let dir = ensure_ok(tempfile::tempdir(), "tempdir should be created")?;
    let path = dir.path().join("perf-test.map");

    ensure_ok(ensure_perf_map_file(&path), "perf map file should be ensured")?;

    ensure(path.is_file(), "perf map path should be a file")?;
    let contents = ensure_ok(std::fs::read_to_string(path), "perf map file should be readable")?;
    ensure_eq(&contents, &String::new(), "new perf map file should be empty")
  }

  #[test]
  fn ensure_perf_map_file_preserves_existing_symbols() -> std::result::Result<(), TestFailure> {
    let dir = ensure_ok(tempfile::tempdir(), "tempdir should be created")?;
    let path = dir.path().join("perf-test.map");
    let symbols = "7f000000 2a sampled function with spaces\n";
    ensure_ok(std::fs::write(&path, symbols), "symbols should be written")?;

    ensure_ok(ensure_perf_map_file(&path), "perf map file should be ensured")?;

    let contents = ensure_ok(std::fs::read_to_string(path), "perf map file should be readable")?;
    ensure_eq(&contents, &symbols.to_owned(), "existing perf map symbols should be preserved")
  }

  #[test]
  fn ensure_perf_map_file_rejects_directory() -> std::result::Result<(), TestFailure> {
    let dir = ensure_ok(tempfile::tempdir(), "tempdir should be created")?;
    let error = ensure_perf_map_file(dir.path()).err();

    ensure_contains(
      &error.as_ref().map(ToString::to_string).unwrap_or_default(),
      "failed to ensure perf map file",
      "ensuring a perf map at a directory path should report perf-map file failure",
    )?;
    let source_message = error
      .as_ref()
      .and_then(std::error::Error::source)
      .map(ToString::to_string)
      .unwrap_or_default();
    ensure(
      !source_message.is_empty(),
      "perf-map file failure should preserve an underlying io diagnostic",
    )
  }

  #[test]
  fn perf_map_parses_symbols_with_spaces_and_open_end_boundaries() -> std::result::Result<(), TestFailure> {
    let perf_map = perf_map_from("7f000000 2a sampled function with spaces\n")?;

    let start_symbol = ensure_some(perf_map.find(0x7f000000), "start address should resolve")?;
    let end_minus_one_symbol = ensure_some(perf_map.find(0x7f000029), "last address before end should resolve")?;

    ensure(
      Symbol::from(start_symbol).name == Some(b"sampled function with spaces".to_vec()),
      "symbol name should preserve spaces",
    )?;
    ensure(
      Symbol::from(end_minus_one_symbol).name == Some(b"sampled function with spaces".to_vec()),
      "end-minus-one address should resolve to same symbol",
    )?;
    ensure(perf_map.find(0x7effffff).is_none(), "address before start should be outside range")?;
    ensure(perf_map.find(0x7f00002a).is_none(), "end address should be outside range")
  }

  #[test]
  fn perf_map_allows_empty_symbol_names() -> std::result::Result<(), TestFailure> {
    let perf_map = perf_map_from("1000 10\n")?;
    let symbol = ensure_some(perf_map.find(0x1000), "range with empty name should resolve")?;

    ensure(
      Symbol::from(symbol).name == Some(Vec::<u8>::new()),
      "empty perf-map names should be preserved as empty symbol names",
    )
  }

  #[test]
  fn perf_map_uses_first_matching_range_for_overlaps() -> std::result::Result<(), TestFailure> {
    let perf_map = perf_map_from("1000 20 first\n1010 20 second\n")?;
    let symbol = ensure_some(perf_map.find(0x1015), "overlapping address should resolve")?;

    ensure(
      Symbol::from(symbol).name == Some(b"first".to_vec()),
      "overlapping ranges should resolve to the first matching perf-map entry",
    )
  }

  #[test]
  fn perf_map_rejects_malformed_lines() -> std::result::Result<(), TestFailure> {
    let missing_length = temp_perf_map_file("7f000000\n")?;
    let invalid_start = temp_perf_map_file("not-hex 2a bad symbol\n")?;
    let invalid_length = temp_perf_map_file("7f000000 not-hex bad symbol\n")?;
    let overflowing_range = temp_perf_map_file(&format!("{:x} 1 overflowing symbol\n", usize::MAX))?;

    ensure(
      PerfMap::new(missing_length).is_none(),
      "perf map lines without a length should reject the whole map",
    )?;
    ensure(
      PerfMap::new(invalid_start).is_none(),
      "perf map lines with invalid starts should reject the whole map",
    )?;
    ensure(
      PerfMap::new(invalid_length).is_none(),
      "perf map lines with invalid lengths should reject the whole map",
    )?;
    ensure(
      PerfMap::new(overflowing_range).is_none(),
      "perf map ranges that overflow usize should reject the whole map",
    )
  }

  #[test]
  fn perf_map_symbol_converts_to_symbol_name_only() -> std::result::Result<(), TestFailure> {
    let symbol = Symbol::from(PerfMapSymbol("jit symbol".to_owned()));

    ensure(
      symbol.name == Some(b"jit symbol".to_vec()),
      "perf-map symbol conversion should move the symbol name into raw bytes",
    )?;
    ensure(symbol.addr.is_none(), "perf-map symbol conversion should not invent addresses")?;
    ensure(symbol.filename.is_none(), "perf-map symbol conversion should not invent filenames")?;
    ensure(symbol.lineno.is_none(), "perf-map symbol conversion should not invent line numbers")
  }

  #[test]
  fn process_perf_map_path_uses_current_process_id() -> std::result::Result<(), TestFailure> {
    let path = process_perf_map_path();
    let filename = ensure_some(
      path.file_name().and_then(|filename| filename.to_str()),
      "process perf-map path should have a utf8 filename",
    )?;
    let expected_filename = format!("perf-{}.map", std::process::id());

    ensure_eq(
      &filename,
      &expected_filename.as_str(),
      "process perf-map path should point at the current process map",
    )
  }

  #[test]
  fn resolver_loads_changed_map_and_skips_unchanged_loaded_mtime() -> std::result::Result<(), TestFailure> {
    let dir = ensure_ok(tempfile::tempdir(), "tempdir should be created")?;
    let path = dir.path().join("perf-test.map");
    let last_loaded_mtime_seconds = AtomicU64::new(0);
    ensure_ok(
      std::fs::write(&path, b"1000 10 jit compiled symbol\n"),
      "perf map should be written",
    )?;

    let perf_map = ensure_some(init_resolver_at(&path, &last_loaded_mtime_seconds), "changed perf map should load")?;

    ensure(perf_map.find(0x100f).is_some(), "loaded resolver should expose mapped addresses")?;
    ensure(perf_map.find(0x1010).is_none(), "loaded resolver should preserve open end boundary")?;
    ensure(
      init_resolver_at(&path, &last_loaded_mtime_seconds).is_none(),
      "unchanged loaded mtime second should skip resolver reload",
    )
  }

  #[test]
  fn resolver_ignores_missing_map_file() -> std::result::Result<(), TestFailure> {
    let dir = ensure_ok(tempfile::tempdir(), "tempdir should be created")?;
    let path = dir.path().join("missing-perf.map");
    let last_loaded_mtime_seconds = AtomicU64::new(0);

    ensure(
      init_resolver_at(&path, &last_loaded_mtime_seconds).is_none(),
      "missing perf map file should not create a resolver",
    )?;
    ensure_eq(
      &last_loaded_mtime_seconds.load(Ordering::Relaxed),
      &0,
      "missing perf map file should not advance last loaded mtime",
    )
  }

  #[test]
  fn resolver_ignores_directory_map_path() -> std::result::Result<(), TestFailure> {
    let dir = ensure_ok(tempfile::tempdir(), "tempdir should be created")?;
    let last_loaded_mtime_seconds = AtomicU64::new(0);

    ensure(
      init_resolver_at(dir.path(), &last_loaded_mtime_seconds).is_none(),
      "directory perf-map paths should not create a resolver",
    )?;
    ensure_eq(
      &last_loaded_mtime_seconds.load(Ordering::Relaxed),
      &0,
      "directory perf-map paths should not advance last loaded mtime",
    )
  }

  #[test]
  fn resolver_retries_after_invalid_map_without_advancing_last_loaded_mtime() -> std::result::Result<(), TestFailure> {
    let dir = ensure_ok(tempfile::tempdir(), "tempdir should be created")?;
    let path = dir.path().join("perf-test.map");
    let last_loaded_mtime_seconds = AtomicU64::new(0);
    ensure_ok(std::fs::write(&path, b"not-hex 10 invalid\n"), "invalid perf map should be written")?;

    ensure(
      init_resolver_at(&path, &last_loaded_mtime_seconds).is_none(),
      "invalid perf map should not create a resolver",
    )?;
    ensure_eq(
      &last_loaded_mtime_seconds.load(Ordering::Relaxed),
      &0,
      "invalid perf map should not advance last loaded mtime",
    )?;

    ensure_ok(
      std::fs::write(&path, b"1000 10 corrected symbol\n"),
      "corrected perf map should be written",
    )?;
    let perf_map = ensure_some(
      init_resolver_at(&path, &last_loaded_mtime_seconds),
      "corrected perf map should load after invalid content",
    )?;
    let symbol = ensure_some(perf_map.find(0x1000), "corrected perf map should resolve start address")?;

    ensure(
      Symbol::from(symbol).name == Some(b"corrected symbol".to_vec()),
      "corrected perf map should expose corrected symbols",
    )
  }

  #[test]
  fn global_perf_map_resolver_exposes_requested_address() -> std::result::Result<(), TestFailure> {
    let _guard = PROCESS_PERF_MAP_LOCK.lock();
    let process_map = ProcessPerfMapGuard::capture()?;
    ensure_ok(
      std::fs::write(&process_map.path, b"1000 10 global resolver symbol\n"),
      "process perf-map should be written",
    )?;
    LAST_LOADED_MTIME_SECONDS.store(0, Ordering::Relaxed);

    refresh_process_resolver();
    let resolver = get_resolver();
    let perf_map = ensure_some(resolver.as_ref().as_ref(), "global perf-map resolver should load process map")?;
    let resolved_symbol = Symbol::from(ensure_some(
      perf_map.find(0x1000),
      "global perf-map resolver should expose the requested address",
    )?)
    .name;

    ensure(
      resolved_symbol == Some(b"global resolver symbol".to_vec()),
      "global perf map resolver should expose the requested address",
    )
  }
}
