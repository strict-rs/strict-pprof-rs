// Copyright 2019 TiKV Project Authors. Licensed under Apache-2.0.

use std::borrow::Cow;
use std::fmt::Debug;
use std::fmt::Display;
use std::fmt::Formatter;
use std::fmt::{
  self,
};
use std::hash::Hash;
use std::hash::Hasher;
use std::os::raw::c_void;
use std::path::PathBuf;
use std::time::SystemTime;

use smallvec::SmallVec;
use symbolic_demangle::demangle;

use crate::MAX_DEPTH;
use crate::MAX_THREAD_NAME;
use crate::backtrace::Frame;
use crate::backtrace::Trace;
use crate::backtrace::TraceImpl;
use crate::backtrace::frame_ip;
use crate::backtrace::frame_symbol_address;

pub(crate) fn current_system_time() -> SystemTime {
  SystemTime::from(chrono::Utc::now())
}

fn copy_thread_name(thread_name_bytes: &[u8]) -> ([u8; MAX_THREAD_NAME], usize) {
  let mut thread_name = [0; MAX_THREAD_NAME];
  let thread_name_length = std::cmp::min(thread_name_bytes.len(), MAX_THREAD_NAME);
  for (destination, source) in thread_name.iter_mut().zip(thread_name_bytes.iter()).take(thread_name_length) {
    *destination = *source;
  }
  (thread_name, thread_name_length)
}

#[cfg(feature = "perfmaps")]
fn resolve_in_perfmap(ip: usize) -> Option<Symbol> {
  use crate::perfmap::get_resolver;

  get_resolver()
    .as_ref()
    .as_ref()
    .and_then(|perf_map_resolver| perf_map_resolver.find(ip))
    .map(Symbol::from)
}

#[cfg(not(feature = "perfmaps"))]
fn resolve_in_perfmap(_ip: usize) -> Option<Symbol> {
  None
}

fn collect_symbols_for_frame<T>(frame: &T) -> Vec<Symbol>
where
  T: Frame,
{
  collect_symbols_for_frame_with_resolver(frame, resolve_in_perfmap)
}

fn collect_symbols_for_frame_with_resolver<T, F>(frame: &T, resolve_symbol_in_perfmap: F) -> Vec<Symbol>
where
  T: Frame,
  F: FnOnce(usize) -> Option<Symbol>,
{
  if let Some(perfmap_symbol) = resolve_symbol_in_perfmap(frame_ip(frame)) {
    return vec![perfmap_symbol];
  }

  let mut symbols = Vec::new();
  frame.resolve_symbol(|symbol| {
    symbols.push(Symbol::from(symbol));
  });
  symbols
}

fn symbols_include_signal_handler(symbols: &[Symbol]) -> bool {
  symbols.iter().any(|symbol| is_signal_handler_symbol_name(&symbol.name()))
}

fn is_signal_handler_symbol_name(symbol_name: &str) -> bool {
  ["perf_signal_handler", "_perf_signal_handler"].contains(&symbol_name) || symbol_name.ends_with("::perf_signal_handler")
}

fn resolved_symbol_frames_from_trace_frames<'a, I, T>(trace_frames: I) -> Vec<Vec<Symbol>>
where
  I: IntoIterator<Item = &'a T>,
  T: Frame + 'a,
{
  let mut symbol_frames = Vec::new();
  let mut frame_iter = trace_frames.into_iter();

  while let Some(frame) = frame_iter.next() {
    let symbols = collect_symbols_for_frame(frame);

    if symbols_include_signal_handler(&symbols) {
      // ignore frame itself and its next one
      frame_iter.next();
      continue;
    }

    if !symbols.is_empty() {
      symbol_frames.push(symbols);
    }
  }

  symbol_frames
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
enum FrameIdentity {
  SymbolAddress(*mut c_void),
  InstructionPointer(usize),
}

fn frame_identity<T>(frame: &T) -> FrameIdentity
where
  T: Frame,
{
  let symbol_address = frame_symbol_address(frame);
  if symbol_address.is_null() {
    FrameIdentity::InstructionPointer(frame_ip(frame))
  } else {
    FrameIdentity::SymbolAddress(symbol_address)
  }
}

fn frame_identities_match<T>(first_frames: &[T], second_frames: &[T]) -> bool
where
  T: Frame,
{
  if first_frames.len() != second_frames.len() {
    return false;
  }

  Iterator::zip(first_frames.iter(), second_frames.iter())
    .all(|(first_frame, second_frame)| frame_identity(first_frame) == frame_identity(second_frame))
}

fn hash_frame_identities<T, H>(frames: &[T], state: &mut H)
where
  T: Frame,
  H: Hasher,
{
  frames.iter().for_each(|frame| frame_identity(frame).hash(state));
}

#[derive(Clone)]
pub struct UnresolvedFrames {
  pub frames:             SmallVec<[<TraceImpl as Trace>::Frame; MAX_DEPTH]>,
  pub thread_name:        [u8; MAX_THREAD_NAME],
  pub thread_name_length: usize,
  pub thread_id:          u64,
  pub sample_timestamp:   SystemTime,
}

impl Default for UnresolvedFrames {
  fn default() -> Self {
    let frames = SmallVec::with_capacity(MAX_DEPTH);
    Self {
      frames,
      thread_name: [0; MAX_THREAD_NAME],
      thread_name_length: 0,
      thread_id: 0,
      sample_timestamp: current_system_time(),
    }
  }
}

impl Debug for UnresolvedFrames {
  fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
    self.frames.fmt(f)
  }
}

impl UnresolvedFrames {
  pub fn new(frames: SmallVec<[<TraceImpl as Trace>::Frame; MAX_DEPTH]>, tn: &[u8], thread_id: u64, sample_timestamp: SystemTime) -> Self {
    let (thread_name, thread_name_length) = copy_thread_name(tn);

    Self {
      frames,
      thread_name,
      thread_name_length,
      thread_id,
      sample_timestamp,
    }
  }
}

impl PartialEq for UnresolvedFrames {
  fn eq(&self, other: &Self) -> bool {
    self.thread_id == other.thread_id && frame_identities_match(&self.frames, &other.frames)
  }
}

impl Eq for UnresolvedFrames {}

impl Hash for UnresolvedFrames {
  fn hash<H: Hasher>(&self, state: &mut H) {
    hash_frame_identities(&self.frames, state);
    self.thread_id.hash(state);
  }
}

/// Symbol is a representation of a function symbol. It contains name and addr of it. If built with
/// debug message, it can also provide line number and filename. The name in it is not demangled.
#[derive(Debug, Clone)]
pub struct Symbol {
  /// This name is raw name of a symbol (which hasn't been demangled).
  pub name: Option<Vec<u8>>,

  /// The address of the function. It is not 100% trustworthy.
  pub addr: Option<*mut c_void>,

  /// Line number of this symbol. If compiled with debug message, you can get it.
  pub lineno: Option<u32>,

  /// Filename of this symbol. If compiled with debug message, you can get it.
  pub filename: Option<PathBuf>,
}

impl Symbol {
  pub fn raw_name(&self) -> &[u8] {
    self.name.as_deref().unwrap_or(b"Unknown")
  }

  pub fn name(&self) -> String {
    demangle(&String::from_utf8_lossy(self.raw_name())).into_owned()
  }

  pub fn sys_name(&self) -> Cow<'_, str> {
    String::from_utf8_lossy(self.raw_name())
  }

  pub fn filename(&self) -> Cow<'_, str> {
    self
      .filename
      .as_ref()
      .map(|name| name.as_os_str().to_string_lossy())
      .unwrap_or_else(|| Cow::Borrowed("Unknown"))
  }

  pub fn lineno(&self) -> u32 {
    self.lineno.unwrap_or(0)
  }
}

unsafe impl Send for Symbol {}

impl<T> From<&T> for Symbol
where
  T: crate::backtrace::Symbol,
{
  fn from(symbol: &T) -> Self {
    Symbol {
      name:     symbol.name(),
      addr:     symbol.addr(),
      lineno:   symbol.lineno(),
      filename: symbol.filename(),
    }
  }
}

impl Display for Symbol {
  fn fmt(&self, f: &mut Formatter) -> fmt::Result {
    f.write_str(&self.name())
  }
}

impl PartialEq for Symbol {
  fn eq(&self, other: &Self) -> bool {
    self.raw_name() == other.raw_name()
  }
}

impl Hash for Symbol {
  fn hash<H: Hasher>(&self, state: &mut H) {
    self.raw_name().hash(state)
  }
}

/// A representation of a backtrace. `thread_name` and `thread_id` was got from `pthread_getname_np`
/// and `pthread_self`. frames is a vector of symbols.
#[derive(Clone, PartialEq, Hash)]
pub struct Frames {
  pub frames:           Vec<Vec<Symbol>>,
  pub thread_name:      String,
  pub thread_id:        u64,
  pub sample_timestamp: SystemTime,
}

impl Frames {
  /// Returns a thread identifier (name or ID) as a string.
  pub fn thread_name_or_id(&self) -> String {
    if !self.thread_name.is_empty() {
      self.thread_name.clone()
    } else {
      format!("{:?}", self.thread_id)
    }
  }
}

impl From<UnresolvedFrames> for Frames {
  fn from(frames: UnresolvedFrames) -> Self {
    let symbol_frames = resolved_symbol_frames_from_trace_frames(frames.frames.iter());

    Self {
      frames:           symbol_frames,
      thread_name:      String::from_utf8_lossy(&frames.thread_name[0..frames.thread_name_length]).into_owned(),
      thread_id:        frames.thread_id,
      sample_timestamp: frames.sample_timestamp,
    }
  }
}

impl Eq for Frames {}

impl Debug for Frames {
  fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
    for frame in self.frames.iter() {
      write!(f, "FRAME: ")?;
      for symbol in frame.iter() {
        write!(f, "{} -> ", symbol)?;
      }
    }
    write!(f, "THREAD: ")?;
    if !self.thread_name.is_empty() {
      write!(f, "{}", self.thread_name)
    } else {
      write!(f, "ThreadId({})", self.thread_id)
    }
  }
}

#[cfg(test)]
mod tests {
  use std::collections::hash_map::DefaultHasher;
  use std::hash::Hash;
  use std::hash::Hasher;
  use std::path::PathBuf;

  use strict_test_support::TestFailure;
  use strict_test_support::ensure;
  use strict_test_support::ensure_contains;
  use strict_test_support::ensure_eq;
  use strict_test_support::ensure_ok;

  use super::*;

  fn plain_symbol(name: Option<&[u8]>) -> Symbol {
    Symbol {
      name:     name.map(<[u8]>::to_vec),
      addr:     None,
      lineno:   None,
      filename: None,
    }
  }

  #[test]
  fn unresolved_frames_default_starts_empty_with_current_timestamp() -> std::result::Result<(), TestFailure> {
    let before = current_system_time();
    let frames = UnresolvedFrames::default();
    let after = current_system_time();

    ensure(frames.frames.is_empty(), "default unresolved frames should not contain frames")?;
    ensure_eq(
      &frames.thread_name_length,
      &0,
      "default unresolved frames should have an empty thread name",
    )?;
    ensure_eq(&frames.thread_id, &0, "default unresolved frames should use thread id zero")?;
    ensure_ok(
      frames.sample_timestamp.duration_since(before),
      "default unresolved frame timestamp should not be earlier than construction start",
    )?;
    ensure_ok(
      after.duration_since(frames.sample_timestamp),
      "default unresolved frame timestamp should not be later than construction end",
    )?;
    Ok(())
  }

  #[test]
  fn unresolved_frames_new_stores_thread_identity_and_timestamp() -> std::result::Result<(), TestFailure> {
    let timestamp = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(7);
    let frames = UnresolvedFrames::new(SmallVec::new(), b"sample-thread", 42, timestamp);

    ensure_eq(&frames.thread_name_length, &13, "thread name length should be stored")?;
    ensure(
      frames.thread_name[..frames.thread_name_length] == b"sample-thread"[..],
      "thread name should be copied",
    )?;
    ensure_eq(&frames.thread_id, &42, "thread id should be stored")?;
    ensure(frames.sample_timestamp == timestamp, "sample timestamp should be stored")
  }

  #[test]
  fn unresolved_frames_new_preserves_empty_and_exact_width_thread_names() -> std::result::Result<(), TestFailure> {
    let empty = Frames::from(UnresolvedFrames::new(SmallVec::new(), b"", 1, SystemTime::UNIX_EPOCH));
    let exact_thread_name = [b'a'; MAX_THREAD_NAME];
    let exact = Frames::from(UnresolvedFrames::new(
      SmallVec::new(),
      &exact_thread_name,
      2,
      SystemTime::UNIX_EPOCH,
    ));

    ensure_eq(&empty.thread_name, &String::new(), "empty thread names should stay empty")?;
    ensure_eq(
      &empty.thread_name_or_id(),
      &"1".to_owned(),
      "unnamed frames should expose the thread id as identity",
    )?;
    ensure_eq(
      &exact.thread_name,
      &"a".repeat(MAX_THREAD_NAME),
      "exact-width thread names should be preserved without truncation",
    )
  }

  #[test]
  fn unresolved_frames_new_truncates_long_thread_names() -> std::result::Result<(), TestFailure> {
    let frames = UnresolvedFrames::new(SmallVec::new(), b"worker-name-that-is-too-long", 42, SystemTime::UNIX_EPOCH);
    let resolved = Frames::from(frames);

    ensure_eq(
      &resolved.thread_name,
      &"worker-name-that".to_owned(),
      "long thread names should be truncated to the fixed profiler buffer",
    )
  }

  #[test]
  fn unresolved_frames_decode_thread_names_lossily() -> std::result::Result<(), TestFailure> {
    let frames = UnresolvedFrames::new(SmallVec::new(), b"worker-\xff", 42, SystemTime::UNIX_EPOCH);
    let resolved = Frames::from(frames);

    ensure_eq(
      &resolved.thread_name,
      &"worker-\u{fffd}".to_owned(),
      "non-utf8 thread names should decode lossily instead of failing report construction",
    )
  }

  #[test]
  fn unresolved_frames_equality_uses_thread_id_not_thread_name() -> std::result::Result<(), TestFailure> {
    let renamed_thread = UnresolvedFrames::new(SmallVec::new(), b"worker-renamed", 1, SystemTime::UNIX_EPOCH);
    let same_thread = UnresolvedFrames::new(SmallVec::new(), b"worker-original", 1, SystemTime::UNIX_EPOCH);
    let different_thread = UnresolvedFrames::new(SmallVec::new(), b"worker-renamed", 2, SystemTime::UNIX_EPOCH);

    ensure(
      renamed_thread == same_thread,
      "unresolved frame identity should use thread id rather than mutable thread names",
    )?;
    ensure(
      renamed_thread != different_thread,
      "unresolved frame equality should keep samples from different threads distinct",
    )
  }

  #[cfg(any(pprof_framehop_backend, pprof_frame_pointer_backend))]
  #[test]
  fn unresolved_frames_equality_distinguishes_frame_addresses_for_same_thread() -> std::result::Result<(), TestFailure> {
    let first_stack = UnresolvedFrames::new(
      SmallVec::from_vec(vec![crate::backtrace::BacktraceFrame {
        ip: 0x1000
      }]),
      b"worker",
      1,
      SystemTime::UNIX_EPOCH,
    );
    let different_stack = UnresolvedFrames::new(
      SmallVec::from_vec(vec![crate::backtrace::BacktraceFrame {
        ip: 0x2000
      }]),
      b"worker",
      1,
      SystemTime::UNIX_EPOCH,
    );

    ensure(
      first_stack != different_stack,
      "unresolved frame equality should distinguish different sampled instruction addresses in the same thread",
    )
  }

  #[test]
  fn unresolved_frames_identity_uses_frame_symbol_addresses_and_frame_count() -> std::result::Result<(), TestFailure> {
    let first_frame = test_frame(0x1000, vec![test_symbol(b"first")]);
    let same_symbol_address = test_frame(0x1000, vec![test_symbol(b"renamed")]);
    let different_symbol_address = test_frame(0x2000, vec![test_symbol(b"first")]);

    ensure(
      frame_identities_match(std::slice::from_ref(&first_frame), std::slice::from_ref(&same_symbol_address)),
      "frame identity should use symbol addresses rather than debug names",
    )?;
    ensure(
      !frame_identities_match(std::slice::from_ref(&first_frame), std::slice::from_ref(&different_symbol_address)),
      "frame identity should distinguish different symbol addresses",
    )?;
    let longer_stack = [same_symbol_address, test_frame(0x1000, vec![test_symbol(b"also-renamed")])];
    ensure(
      !frame_identities_match(std::slice::from_ref(&first_frame), &longer_stack),
      "frame identity should distinguish different stack depths",
    )
  }

  #[test]
  fn unresolved_frames_identity_falls_back_to_instruction_pointer_when_symbol_address_is_null() -> std::result::Result<(), TestFailure> {
    let first_frame = NullSymbolAddressFrame {
      ip: 0x1000
    };
    let same_instruction_pointer = NullSymbolAddressFrame {
      ip: 0x1000
    };
    let different_instruction_pointer = NullSymbolAddressFrame {
      ip: 0x2000
    };

    ensure(
      frame_identities_match(std::slice::from_ref(&first_frame), std::slice::from_ref(&same_instruction_pointer)),
      "null symbol addresses should fall back to matching instruction pointers",
    )?;
    ensure(
      !frame_identities_match(
        std::slice::from_ref(&first_frame),
        std::slice::from_ref(&different_instruction_pointer),
      ),
      "null symbol addresses should not collapse distinct instruction pointers",
    )
  }

  #[test]
  fn unresolved_frames_hash_matches_frame_identity() -> std::result::Result<(), TestFailure> {
    let first = [test_frame(0x1000, vec![test_symbol(b"first")])];
    let equivalent = [test_frame(0x1000, vec![test_symbol(b"renamed")])];
    let mut first_hash = DefaultHasher::new();
    let mut equivalent_hash = DefaultHasher::new();

    hash_frame_identities(&first, &mut first_hash);
    hash_frame_identities(&equivalent, &mut equivalent_hash);

    ensure(
      frame_identities_match(&first, &equivalent),
      "test setup should construct equal frame identities",
    )?;
    ensure_eq(
      &first_hash.finish(),
      &equivalent_hash.finish(),
      "equal unresolved frame identities should hash equally",
    )
  }

  #[test]
  fn frames_from_empty_unresolved_frames_preserves_thread_metadata() -> std::result::Result<(), TestFailure> {
    let timestamp = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(9);
    let frames = Frames::from(UnresolvedFrames::new(SmallVec::new(), b"worker", 11, timestamp));

    ensure(frames.frames.is_empty(), "empty unresolved frames should produce no symbol frames")?;
    ensure_eq(&frames.thread_name, &"worker".to_owned(), "thread name should be decoded")?;
    ensure_eq(&frames.thread_id, &11, "thread id should be preserved")?;
    ensure(frames.sample_timestamp == timestamp, "sample timestamp should be preserved")
  }

  #[test]
  fn symbol_fallbacks_are_stable_for_unknown_debug_metadata() -> std::result::Result<(), TestFailure> {
    let unknown = plain_symbol(None);
    let with_file = Symbol {
      filename: Some("src/lib.rs".into()),
      lineno: Some(42),
      ..plain_symbol(Some(b"sampled_function"))
    };

    ensure(unknown.raw_name() == b"Unknown", "missing symbol name should use Unknown")?;
    ensure_eq(&unknown.name(), &"Unknown".to_owned(), "missing name should display as Unknown")?;
    ensure_eq(&unknown.sys_name().as_ref(), &"Unknown", "missing system name should use Unknown")?;
    ensure_eq(&unknown.filename().as_ref(), &"Unknown", "missing filename should use Unknown")?;
    ensure_eq(&unknown.lineno(), &0, "missing line number should use zero")?;
    ensure_eq(&with_file.filename().as_ref(), &"src/lib.rs", "filename should be stringified")?;
    ensure_eq(&with_file.lineno(), &42, "line number should be returned")
  }

  #[test]
  fn symbols_compare_and_hash_by_raw_name() -> std::result::Result<(), TestFailure> {
    let first = plain_symbol(Some(b"same"));
    let second = plain_symbol(Some(b"same"));
    let different = plain_symbol(Some(b"different"));
    let mut first_hash = DefaultHasher::new();
    let mut second_hash = DefaultHasher::new();

    first.hash(&mut first_hash);
    second.hash(&mut second_hash);

    ensure(first == second, "symbols with same raw name should compare equal")?;
    ensure(first != different, "symbols with different raw names should not compare equal")?;
    ensure_eq(&first_hash.finish(), &second_hash.finish(), "equal symbols should hash equally")
  }

  #[test]
  fn frames_thread_identity_and_debug_output_use_name_or_id() -> std::result::Result<(), TestFailure> {
    let named = Frames {
      frames:           vec![vec![plain_symbol(Some(b"sampled_function"))]],
      thread_name:      "worker-thread".to_owned(),
      thread_id:        7,
      sample_timestamp: SystemTime::UNIX_EPOCH,
    };
    let unnamed = Frames {
      frames:           Vec::new(),
      thread_name:      String::new(),
      thread_id:        8,
      sample_timestamp: SystemTime::UNIX_EPOCH,
    };

    ensure_eq(
      &named.thread_name_or_id(),
      &"worker-thread".to_owned(),
      "thread name should be preferred",
    )?;
    ensure_eq(
      &unnamed.thread_name_or_id(),
      &"8".to_owned(),
      "thread id should be used when name is empty",
    )?;
    ensure_contains(
      &format!("{named:?}"),
      "FRAME: sampled_function -> ",
      "debug output should include symbol frames",
    )?;
    ensure_contains(
      &format!("{unnamed:?}"),
      "ThreadId(8)",
      "debug output should include unnamed thread id",
    )
  }

  #[derive(Clone)]
  struct TestBacktraceSymbol {
    name:     Option<Vec<u8>>,
    addr:     Option<*mut c_void>,
    lineno:   Option<u32>,
    filename: Option<PathBuf>,
  }

  impl crate::backtrace::Symbol for TestBacktraceSymbol {
    fn name(&self) -> Option<Vec<u8>> {
      self.name.clone()
    }

    fn addr(&self) -> Option<*mut c_void> {
      self.addr
    }

    fn lineno(&self) -> Option<u32> {
      self.lineno
    }

    fn filename(&self) -> Option<PathBuf> {
      self.filename.clone()
    }
  }

  #[derive(Clone)]
  struct TestFrame {
    ip:      usize,
    symbols: Vec<TestBacktraceSymbol>,
  }

  #[derive(Clone)]
  struct NullSymbolAddressFrame {
    ip: usize,
  }

  fn test_symbol(name: &[u8]) -> TestBacktraceSymbol {
    TestBacktraceSymbol {
      name:     Some(name.to_vec()),
      addr:     None,
      lineno:   None,
      filename: None,
    }
  }

  fn test_frame(ip: usize, symbols: Vec<TestBacktraceSymbol>) -> TestFrame {
    TestFrame {
      ip,
      symbols,
    }
  }

  impl Frame for TestFrame {
    type S = TestBacktraceSymbol;

    fn resolve_symbol<F: FnMut(&Self::S)>(&self, mut cb: F) {
      for symbol in &self.symbols {
        cb(symbol);
      }
    }

    fn symbol_address(&self) -> *mut c_void {
      self.ip as *mut c_void
    }

    fn ip(&self) -> usize {
      self.ip
    }
  }

  impl Frame for NullSymbolAddressFrame {
    type S = TestBacktraceSymbol;

    fn resolve_symbol<F: FnMut(&Self::S)>(&self, _: F) {}

    fn symbol_address(&self) -> *mut c_void {
      std::ptr::null_mut()
    }

    fn ip(&self) -> usize {
      self.ip
    }
  }

  #[test]
  fn collect_symbols_uses_frame_symbols_when_perfmap_misses() -> std::result::Result<(), TestFailure> {
    let frame = test_frame(0xdead_beef, vec![TestBacktraceSymbol {
      name:     Some(b"collected_symbol".to_vec()),
      addr:     Some(0x10usize as *mut c_void),
      lineno:   Some(99),
      filename: Some("src/frames.rs".into()),
    }]);

    let symbols = collect_symbols_for_frame(&frame);

    ensure_eq(&symbols.len(), &1, "one frame symbol should be collected")?;
    ensure_eq(
      &symbols[0].name(),
      &"collected_symbol".to_owned(),
      "symbol name should be collected",
    )?;
    ensure_eq(&symbols[0].lineno(), &99, "symbol line should be collected")?;
    ensure_eq(
      &symbols[0].filename().as_ref(),
      &"src/frames.rs",
      "symbol filename should be collected",
    )
  }

  #[test]
  fn collect_symbols_prefers_perfmap_symbol_when_resolver_hits() -> std::result::Result<(), TestFailure> {
    let frame = test_frame(0xdead_beef, vec![test_symbol(b"frame_symbol")]);
    let mut resolver_ip = None;

    let symbols = collect_symbols_for_frame_with_resolver(&frame, |ip| {
      resolver_ip = Some(ip);
      Some(Symbol {
        name:     Some(b"perfmap_symbol".to_vec()),
        addr:     None,
        lineno:   None,
        filename: None,
      })
    });

    ensure(
      resolver_ip == Some(0xdead_beef),
      "perf-map resolver should receive frame instruction pointer",
    )?;
    ensure_eq(&symbols.len(), &1, "perf-map hit should produce one symbol")?;
    ensure_eq(
      &symbols[0].name(),
      &"perfmap_symbol".to_owned(),
      "perf-map symbol should take precedence over frame debug symbols",
    )
  }

  #[test]
  fn collect_symbols_returns_empty_when_frame_has_no_symbols() -> std::result::Result<(), TestFailure> {
    let frame = test_frame(0xbeef_dead, Vec::new());

    ensure(
      collect_symbols_for_frame(&frame).is_empty(),
      "frame with no resolved symbols should produce no symbols",
    )
  }

  #[test]
  fn frames_from_trace_frames_filters_signal_handler_and_following_frame() -> std::result::Result<(), TestFailure> {
    let trace_frames = [
      test_frame(1, vec![test_symbol(b"perf_signal_handler")]),
      test_frame(2, vec![test_symbol(b"skipped_after_signal")]),
      test_frame(3, vec![test_symbol(b"kept_sample")]),
    ];

    let symbol_frames = resolved_symbol_frames_from_trace_frames(trace_frames.iter());

    ensure_eq(&symbol_frames.len(), &1, "signal handler and following frame should be skipped")?;
    ensure_eq(
      &symbol_frames[0][0].name(),
      &"kept_sample".to_owned(),
      "frame after the skipped signal pair should be preserved",
    )
  }

  #[test]
  fn frames_from_trace_frames_discards_frames_without_symbols() -> std::result::Result<(), TestFailure> {
    let trace_frames = [test_frame(1, Vec::new()), test_frame(2, vec![test_symbol(b"resolved_symbol")])];

    let symbol_frames = resolved_symbol_frames_from_trace_frames(trace_frames.iter());

    ensure_eq(&symbol_frames.len(), &1, "frames without symbols should be omitted")?;
    ensure_eq(
      &symbol_frames[0][0].name(),
      &"resolved_symbol".to_owned(),
      "resolved frames should remain after empty frames are omitted",
    )
  }

  #[test]
  fn symbol_from_backtrace_symbol_copies_all_metadata() -> std::result::Result<(), TestFailure> {
    let mut address_anchor = 0u8;
    let symbol_address = std::ptr::addr_of_mut!(address_anchor).cast::<c_void>();
    let backtrace_symbol = TestBacktraceSymbol {
      name:     Some(b"sampled_symbol".to_vec()),
      addr:     Some(symbol_address),
      lineno:   Some(42),
      filename: Some(PathBuf::from("src/lib.rs")),
    };

    let symbol = Symbol::from(&backtrace_symbol);

    ensure(
      symbol.name == Some(b"sampled_symbol".to_vec()),
      "symbol conversion should copy raw name bytes",
    )?;
    ensure(symbol.addr == Some(symbol_address), "symbol conversion should copy addresses")?;
    ensure(symbol.lineno == Some(42), "symbol conversion should copy line numbers")?;
    ensure(
      symbol.filename == Some(PathBuf::from("src/lib.rs")),
      "symbol conversion should copy filenames",
    )
  }

  #[test]
  fn symbol_display_renders_demangled_name() -> std::result::Result<(), TestFailure> {
    let symbol = plain_symbol(Some(b"_ZN3foo3barE"));

    ensure_eq(
      &format!("{symbol}"),
      &"foo::bar".to_owned(),
      "symbol display should render the demangled display name",
    )
  }

  #[test]
  fn signal_handler_symbols_are_detected_for_stack_filtering() -> std::result::Result<(), TestFailure> {
    ensure(
      symbols_include_signal_handler(&[plain_symbol(Some(b"perf_signal_handler"))]),
      "linux signal handler symbol should be detected",
    )?;
    ensure(
      symbols_include_signal_handler(&[plain_symbol(Some(b"_perf_signal_handler"))]),
      "macos signal handler symbol should be detected",
    )?;
    ensure(
      symbols_include_signal_handler(&[plain_symbol(Some(b"pprof::profiler::perf_signal_handler"))]),
      "rust signal handler path should be detected",
    )?;
    ensure(
      !symbols_include_signal_handler(&[plain_symbol(Some(b"sampled_function"))]),
      "ordinary sampled symbols should not be treated as signal handler frames",
    )?;
    ensure(
      !symbols_include_signal_handler(&[plain_symbol(Some(b"application::not_perf_signal_handler"))]),
      "symbols that merely contain the handler name should not be filtered",
    )
  }

  #[test]
  fn demangle_rust() -> std::result::Result<(), TestFailure> {
    let symbol = Symbol {
      name:     Some(b"_ZN3foo3barE".to_vec()),
      addr:     None,
      lineno:   None,
      filename: None,
    };

    ensure(symbol.name() == "foo::bar", "rust symbol should demangle")
  }

  #[cfg(feature = "cpp")]
  #[test]
  fn demangle_cpp_when_cpp_feature_enabled() -> std::result::Result<(), TestFailure> {
    let name = b"_ZNK3MapI10StringName3RefI8GDScriptE10ComparatorIS0_E16DefaultAllocatorE3hasERKS0_".to_vec();

    let symbol = Symbol {
      name:     Some(name),
      addr:     None,
      lineno:   None,
      filename: None,
    };

    ensure(
      symbol.name() == "Map<StringName, Ref<GDScript>, Comparator<StringName>, DefaultAllocator>::has(StringName const&) const",
      "cpp symbol should demangle when cpp feature is enabled",
    )
  }

  #[cfg(not(feature = "cpp"))]
  #[test]
  fn keeps_cpp_symbol_mangled_without_cpp_feature() -> std::result::Result<(), TestFailure> {
    let raw_name = "_ZNK3MapI10StringName3RefI8GDScriptE10ComparatorIS0_E16DefaultAllocatorE3hasERKS0_";
    let symbol = Symbol {
      name:     Some(raw_name.as_bytes().to_vec()),
      addr:     None,
      lineno:   None,
      filename: None,
    };

    ensure(symbol.name() == raw_name, "cpp symbol should stay mangled without cpp feature")
  }
}
