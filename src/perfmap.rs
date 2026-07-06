use std::fs::File;
use std::io::BufRead;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

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
      let name = parts.collect::<Vec<_>>().join(" ");
      ranges.push((start, start + len, name));
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
  fn from(value: PerfMapSymbol) -> Self {
    Symbol {
      name:     Some(value.0.into_bytes()),
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
    .map_err(|_| Error::CreatingError)?;
  Ok(())
}

static MTIME: AtomicU64 = AtomicU64::new(0);

fn init_resolver() -> Option<PerfMap> {
  let path = PathBuf::from("/tmp/").join(format!("perf-{}.map", std::process::id()));
  File::open(&path).ok().and_then(|f| {
    let mtime = path.metadata().ok()?.modified().ok()?;
    let mtime = mtime.duration_since(std::time::UNIX_EPOCH).ok()?;
    if MTIME.load(std::sync::atomic::Ordering::Relaxed) == mtime.as_secs() {
      return None;
    } else {
      MTIME.store(mtime.as_secs(), std::sync::atomic::Ordering::Relaxed);
    }
    PerfMap::new(f)
  })
}

pub fn get_resolver() -> Arc<Option<PerfMap>> {
  static RESOLVER: Lazy<ArcSwap<Option<PerfMap>>> = Lazy::new(|| {
    // this makes sure the file exists
    ensure_perf_map_file(&PathBuf::from("/tmp/").join(format!("perf-{}.map", std::process::id()))).ok();
    ArcSwap::from(Arc::new(init_resolver()))
  });

  std::thread::spawn(|| {
    let perf_map = init_resolver();

    if perf_map.is_none() {
      return;
    }

    RESOLVER.store(Arc::new(perf_map));
  });

  RESOLVER.load().clone()
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn ensure_perf_map_file_creates_missing_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("perf-test.map");

    ensure_perf_map_file(&path).unwrap();

    assert!(path.is_file());
    assert_eq!(std::fs::read_to_string(path).unwrap(), "");
  }

  #[test]
  fn ensure_perf_map_file_preserves_existing_symbols() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("perf-test.map");
    let symbols = "7f000000 2a sampled function with spaces\n";
    std::fs::write(&path, symbols).unwrap();

    ensure_perf_map_file(&path).unwrap();

    assert_eq!(std::fs::read_to_string(path).unwrap(), symbols);
  }
}
