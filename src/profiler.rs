// Copyright 2019 TiKV Project Authors. Licensed under Apache-2.0.

use std::convert::TryInto;
use std::os::raw::c_int;
use std::time::SystemTime;

#[cfg(any(
  target_arch = "x86_64",
  target_arch = "aarch64",
  target_arch = "riscv64",
  target_arch = "loongarch64"
))]
use findshlibs::Segment;
#[cfg(any(
  target_arch = "x86_64",
  target_arch = "aarch64",
  target_arch = "riscv64",
  target_arch = "loongarch64"
))]
use findshlibs::SharedLibrary;
#[cfg(any(
  target_arch = "x86_64",
  target_arch = "aarch64",
  target_arch = "riscv64",
  target_arch = "loongarch64"
))]
use findshlibs::TargetSharedLibrary;
use nix::sys::signal;
use once_cell::sync::Lazy;
use smallvec::SmallVec;
use spin::RwLock;

use crate::MAX_DEPTH;
use crate::MAX_THREAD_NAME;
use crate::backtrace::Trace;
use crate::backtrace::TraceImpl;
use crate::collector::Collector;
use crate::error::Error;
use crate::error::Result;
use crate::frames::UnresolvedFrames;
use crate::frames::current_system_time;
use crate::report::ReportBuilder;
use crate::timer::Timer;

pub(crate) static PROFILER: Lazy<RwLock<Result<Profiler>>> = Lazy::new(|| RwLock::new(Profiler::new()));

#[cfg(test)]
pub(crate) static PROFILER_SIGNAL_TEST_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

pub struct Profiler {
  pub(crate) data: Collector<UnresolvedFrames>,
  sample_counter:  i32,

  old_sigaction: Option<signal::SigAction>,
  running:       bool,

  #[cfg(pprof_frame_pointer_backend)]
  on_stack: bool,

  blocklist_segments: BlocklistSegments,
}

#[derive(Clone)]
pub struct ProfilerGuardBuilder {
  frequency: c_int,

  #[cfg(pprof_frame_pointer_backend)]
  on_stack: bool,

  blocklist_segments: BlocklistSegments,
}

#[derive(Clone, Default)]
struct BlocklistSegments {
  #[cfg(any(
    target_arch = "x86_64",
    target_arch = "aarch64",
    target_arch = "riscv64",
    target_arch = "loongarch64"
  ))]
  ranges: Vec<(usize, usize)>,
}

impl BlocklistSegments {
  #[cfg(any(
    target_arch = "x86_64",
    target_arch = "aarch64",
    target_arch = "riscv64",
    target_arch = "loongarch64"
  ))]
  fn collect<T: AsRef<str>>(blocklist: &[T]) -> Self {
    Self {
      ranges: collect_blocklisted_segments(blocklist),
    }
  }

  #[cfg(any(
    target_arch = "x86_64",
    target_arch = "aarch64",
    target_arch = "riscv64",
    target_arch = "loongarch64"
  ))]
  fn contains(&self, addr: usize) -> bool {
    address_in_segments(addr, &self.ranges)
  }
}

impl Default for ProfilerGuardBuilder {
  fn default() -> ProfilerGuardBuilder {
    ProfilerGuardBuilder {
      frequency: 99,

      #[cfg(pprof_frame_pointer_backend)]
      on_stack:                                     false,

      blocklist_segments: BlocklistSegments::default(),
    }
  }
}

impl ProfilerGuardBuilder {
  pub fn frequency(self, frequency: c_int) -> Self {
    Self {
      frequency,
      ..self
    }
  }

  #[cfg(pprof_frame_pointer_backend)]
  /// Sets whether to use an alternate signal stack via `SA_ONSTACK`.
  ///
  /// This is only available and only works correctly when the `frame-pointer` feature is enabled.
  ///
  /// The `backtrace-rs` unwinder ignores the signal context and unwinds the current stack. Using
  /// an alternate stack with it would produce meaningless results. The `frame-pointer` unwinder,
  /// however, uses the provided `ucontext` to correctly walk the original application stack.
  ///
  /// This should be enabled when the profiler is used in an environment
  /// with small stacks (e.g., inside a Go program) to prevent stack overflow.
  pub fn on_stack(self, on_stack: bool) -> Self {
    Self {
      on_stack,
      ..self
    }
  }

  #[cfg(any(
    target_arch = "x86_64",
    target_arch = "aarch64",
    target_arch = "riscv64",
    target_arch = "loongarch64"
  ))]
  pub fn blocklist<T: AsRef<str>>(self, blocklist: &[T]) -> Self {
    let blocklist_segments = BlocklistSegments::collect(blocklist);

    Self {
      blocklist_segments,
      ..self
    }
  }
  pub fn build(self) -> Result<ProfilerGuard<'static>> {
    if self.frequency <= 0 {
      return Err(Error::InvalidFrequency(self.frequency));
    }

    trigger_lazy();

    match PROFILER.write().as_mut() {
      Err(err) => {
        log::error!("error initializing profiler: {}", err);
        Err(Error::ProfilerInitialization)
      }
      Ok(profiler) => {
        #[cfg(pprof_frame_pointer_backend)]
        {
          profiler.on_stack = self.on_stack;
        }

        profiler.blocklist_segments = self.blocklist_segments;

        match profiler.start() {
          Ok(()) => Ok(ProfilerGuard::<'static> {
            profiler: &PROFILER,
            timer:    Some(Timer::new(self.frequency)),
          }),
          Err(err) => Err(err),
        }
      }
    }
  }
}

#[cfg(any(
  target_arch = "x86_64",
  target_arch = "aarch64",
  target_arch = "riscv64",
  target_arch = "loongarch64"
))]
fn shared_library_matches_blocklist<T, L>(shlib: &L, blocklist: &[T]) -> bool
where
  T: AsRef<str>,
  L: SharedLibrary,
{
  library_name_matches_blocklist(shlib.name().to_str(), blocklist)
}

#[cfg(any(
  target_arch = "x86_64",
  target_arch = "aarch64",
  target_arch = "riscv64",
  target_arch = "loongarch64"
))]
fn library_name_matches_blocklist<T: AsRef<str>>(name: Option<&str>, blocklist: &[T]) -> bool {
  let Some(name) = name else {
    return false;
  };

  blocklist.iter().any(|blocked_name| name.contains(blocked_name.as_ref()))
}

#[cfg(any(
  target_arch = "x86_64",
  target_arch = "aarch64",
  target_arch = "riscv64",
  target_arch = "loongarch64"
))]
fn collect_blocklisted_segments<T: AsRef<str>>(blocklist: &[T]) -> Vec<(usize, usize)> {
  let mut blocklist_segments = Vec::new();
  TargetSharedLibrary::each(|shlib| {
    append_blocklisted_library_segments(shlib, blocklist, &mut blocklist_segments);
  });
  blocklist_segments
}

#[cfg(any(
  target_arch = "x86_64",
  target_arch = "aarch64",
  target_arch = "riscv64",
  target_arch = "loongarch64"
))]
fn append_blocklisted_library_segments<T, L>(shlib: &L, blocklist: &[T], blocklist_segments: &mut Vec<(usize, usize)>)
where
  T: AsRef<str>,
  L: SharedLibrary,
{
  if shared_library_matches_blocklist(shlib, blocklist) {
    append_shared_library_segments(shlib, blocklist_segments);
  }
}

#[cfg(any(
  target_arch = "x86_64",
  target_arch = "aarch64",
  target_arch = "riscv64",
  target_arch = "loongarch64"
))]
fn append_shared_library_segments<L: SharedLibrary>(shlib: &L, blocklist_segments: &mut Vec<(usize, usize)>) {
  for segment in shlib.segments() {
    let start = segment.actual_virtual_memory_address(shlib).0;
    let end = start + segment.len();
    blocklist_segments.push((start, end));
  }
}

#[cfg(any(
  target_arch = "x86_64",
  target_arch = "aarch64",
  target_arch = "riscv64",
  target_arch = "loongarch64"
))]
fn address_in_segments(addr: usize, segments: &[(usize, usize)]) -> bool {
  segments.iter().any(|(start, end)| addr > *start && addr < *end)
}

/// RAII structure used to stop profiling when dropped. It is the only interface to access profiler.
pub struct ProfilerGuard<'a> {
  profiler: &'a RwLock<Result<Profiler>>,
  timer:    Option<Timer>,
}

fn trigger_lazy() {
  let _ = backtrace::Backtrace::new();
  let _profiler = PROFILER.read();
  TraceImpl::init();
}

impl ProfilerGuard<'_> {
  /// Start profiling with given sample frequency.
  pub fn new(frequency: c_int) -> Result<ProfilerGuard<'static>> {
    ProfilerGuardBuilder::default().frequency(frequency).build()
  }

  /// Generate a report
  pub fn report(&self) -> ReportBuilder<'_> {
    ReportBuilder::new(self.profiler, self.timer.as_ref().map(Timer::timing).unwrap_or_default())
  }
}

impl<'a> Drop for ProfilerGuard<'a> {
  fn drop(&mut self) {
    drop(self.timer.take());

    match self.profiler.write().as_mut() {
      Err(_) => {}
      Ok(profiler) => match profiler.stop() {
        Ok(()) => {}
        Err(err) => log::error!("error while stopping profiler {}", err),
      },
    }
  }
}

fn write_thread_name_fallback(current_thread: libc::pthread_t, name: &mut [libc::c_char]) {
  // Decimal digits needed to represent any `u128` value.
  const MAX_U128_DECIMAL_DIGITS: usize = 39;

  let mut thread_id = current_thread as u128;
  let mut digits = [0_u8; MAX_U128_DECIMAL_DIGITS];
  let mut digit_count = 0;

  while thread_id > 0 && digit_count < digits.len() {
    digits[digit_count] = match u8::try_from(thread_id % 10) {
      Ok(digit) => digit,
      Err(_) => {
        log::error!("fail to convert thread_id digit to string");
        0
      }
    };
    thread_id /= 10;
    digit_count = digit_count.saturating_add(1);
  }

  let len = std::cmp::min(digit_count, std::cmp::min(name.len(), MAX_THREAD_NAME));
  let mut index = 0;
  while index < len {
    let source_index = match digit_count.checked_sub(1).and_then(|last_digit| last_digit.checked_sub(index)) {
      Some(source_index) => source_index,
      None => {
        break;
      }
    };

    name[index] = match digits[source_index]
      .checked_add(b'0')
      .and_then(|ascii_digit| ascii_digit.try_into().ok())
    {
      Some(digit) => digit,
      None => {
        log::error!("fail to convert thread_id to string");
        0
      }
    };

    index = index.saturating_add(1);
  }
}

#[cfg(not(all(any(target_os = "linux", target_os = "macos"), target_env = "gnu")))]
fn write_thread_name(current_thread: libc::pthread_t, name: &mut [libc::c_char]) {
  write_thread_name_fallback(current_thread, name);
}

#[cfg(all(any(target_os = "linux", target_os = "macos"), target_env = "gnu"))]
fn write_thread_name(current_thread: libc::pthread_t, name: &mut [libc::c_char]) {
  let name_ptr = name as *mut [libc::c_char] as *mut libc::c_char;
  let native_lookup_result = unsafe { libc::pthread_getname_np(current_thread, name_ptr, MAX_THREAD_NAME) };

  write_thread_name_after_native_lookup(current_thread, name, native_lookup_result);
}

#[cfg(all(any(target_os = "linux", target_os = "macos"), target_env = "gnu"))]
fn write_thread_name_after_native_lookup(current_thread: libc::pthread_t, name: &mut [libc::c_char], native_lookup_result: libc::c_int) {
  if native_lookup_result != 0 {
    write_thread_name_fallback(current_thread, name);
  }
}

fn current_errno() -> libc::c_int {
  unsafe {
    cfg_if::cfg_if! {
      if #[cfg(target_os = "android")] {
        *libc::__errno()
      } else if #[cfg(target_os = "linux")] {
        *libc::__errno_location()
      } else if #[cfg(any(target_os = "macos", target_os = "freebsd"))] {
        *libc::__error()
      } else {
        0
      }
    }
  }
}

fn set_current_errno(errno: libc::c_int) {
  unsafe {
    cfg_if::cfg_if! {
      if #[cfg(target_os = "android")] {
        *libc::__errno() = errno;
      } else if #[cfg(target_os = "linux")] {
        *libc::__errno_location() = errno;
      } else if #[cfg(any(target_os = "macos", target_os = "freebsd"))] {
        *libc::__error() = errno;
      } else {
        let _ = errno;
      }
    }
  }
}

struct ErrnoProtector(libc::c_int);

impl ErrnoProtector {
  fn new() -> Self {
    Self(current_errno())
  }
}

impl Drop for ErrnoProtector {
  fn drop(&mut self) {
    set_current_errno(self.0);
  }
}

#[cfg(any(
  target_arch = "x86_64",
  target_arch = "aarch64",
  target_arch = "riscv64",
  target_arch = "loongarch64"
))]
fn signal_context_is_blocklisted(profiler: &Profiler, ucontext: *mut libc::c_void) -> bool {
  signal_context_instruction_pointer(ucontext).is_some_and(|addr| profiler.is_blocklisted(addr))
}

#[cfg(any(
  target_arch = "x86_64",
  target_arch = "aarch64",
  target_arch = "riscv64",
  target_arch = "loongarch64"
))]
fn signal_context_instruction_pointer(ucontext: *mut libc::c_void) -> Option<usize> {
  let ucontext = ucontext.cast::<libc::ucontext_t>();
  if ucontext.is_null() {
    return None;
  }

  cfg_if::cfg_if! {
    if #[cfg(all(target_arch = "x86_64", target_os = "linux"))] {
      let register_index = usize::try_from(libc::REG_RIP).ok()?;
      unsafe { (*ucontext).uc_mcontext.gregs[register_index].try_into().ok() }
    } else if #[cfg(all(target_arch = "x86_64", target_os = "freebsd"))] {
      unsafe { (*ucontext).uc_mcontext.mc_rip.try_into().ok() }
    } else if #[cfg(all(target_arch = "x86_64", target_os = "macos"))] {
      unsafe {
        let mcontext = (*ucontext).uc_mcontext;
        if mcontext.is_null() {
          None
        } else {
          (*mcontext).__ss.__rip.try_into().ok()
        }
      }
    } else if #[cfg(all(target_arch = "aarch64", any(target_os = "android", target_os = "linux")))] {
      unsafe { (*ucontext).uc_mcontext.pc.try_into().ok() }
    } else if #[cfg(all(target_arch = "aarch64", target_os = "freebsd"))] {
      unsafe { (*ucontext).mc_gpregs.gp_elr.try_into().ok() }
    } else if #[cfg(all(target_arch = "aarch64", target_os = "macos"))] {
      unsafe {
        let mcontext = (*ucontext).uc_mcontext;
        if mcontext.is_null() {
          None
        } else {
          (*mcontext).__ss.__pc.try_into().ok()
        }
      }
    } else if #[cfg(all(target_arch = "riscv64", target_os = "linux"))] {
      unsafe { (*ucontext).uc_mcontext.__gregs[libc::REG_PC].try_into().ok() }
    } else if #[cfg(all(target_arch = "loongarch64", target_os = "linux"))] {
      unsafe { (*ucontext).uc_mcontext.__pc.try_into().ok() }
    } else {
      None
    }
  }
}

extern "C" fn perf_signal_handler(_signal: c_int, _siginfo: *mut libc::siginfo_t, ucontext: *mut libc::c_void) {
  let _errno = ErrnoProtector::new();

  let Some(mut guard) = PROFILER.try_write() else {
    return;
  };
  let Ok(profiler) = guard.as_mut() else {
    return;
  };

  #[cfg(any(
    target_arch = "x86_64",
    target_arch = "aarch64",
    target_arch = "riscv64",
    target_arch = "loongarch64"
  ))]
  if signal_context_is_blocklisted(profiler, ucontext) {
    return;
  }

  let mut bt: SmallVec<[<TraceImpl as Trace>::Frame; MAX_DEPTH]> = SmallVec::with_capacity(MAX_DEPTH);
  let sample_timestamp = current_system_time();
  TraceImpl::trace(ucontext, |frame| {
    #[cfg(pprof_frame_pointer_backend)]
    {
      let ip = crate::backtrace::Frame::ip(frame);
      if profiler.is_blocklisted(ip) {
        return false;
      }
    }

    push_frame_within_max_depth(&mut bt, frame)
  });

  let current_thread = unsafe { libc::pthread_self() };
  let mut name = [0; MAX_THREAD_NAME];
  let name_ptr = &mut name as *mut [libc::c_char] as *mut libc::c_char;

  write_thread_name(current_thread, &mut name);

  let name = unsafe { std::ffi::CStr::from_ptr(name_ptr) };
  profiler.sample(bt, name.to_bytes(), current_thread as u64, sample_timestamp);
}

fn push_frame_within_max_depth<T>(frames: &mut SmallVec<[T; MAX_DEPTH]>, frame: &T) -> bool
where
  T: Clone,
{
  if frames.len() < MAX_DEPTH {
    frames.push(frame.clone());
    true
  } else {
    false
  }
}

impl Profiler {
  pub(crate) fn new() -> Result<Self> {
    Ok(Profiler {
      data:           Collector::new()?,
      sample_counter: 0,
      old_sigaction:  None,
      running:        false,

      #[cfg(pprof_frame_pointer_backend)]
      on_stack:                                     false,

      blocklist_segments: BlocklistSegments::default(),
    })
  }

  #[cfg(any(
    target_arch = "x86_64",
    target_arch = "aarch64",
    target_arch = "riscv64",
    target_arch = "loongarch64"
  ))]
  fn is_blocklisted(&self, addr: usize) -> bool {
    self.blocklist_segments.contains(addr)
  }
}

impl Profiler {
  pub fn start(&mut self) -> Result<()> {
    log::info!("starting cpu profiler");
    if self.running {
      Err(Error::AlreadyRunning)
    } else {
      self.register_signal_handler()?;
      self.running = true;

      Ok(())
    }
  }

  fn init(&mut self) -> Result<()> {
    self.sample_counter = 0;
    self.data = Collector::new()?;
    self.running = false;

    Ok(())
  }

  pub fn stop(&mut self) -> Result<()> {
    log::info!("stopping cpu profiler");
    if self.running {
      self.unregister_signal_handler()?;
      self.init()?;

      Ok(())
    } else {
      Err(Error::NotRunning)
    }
  }

  fn register_signal_handler(&mut self) -> Result<()> {
    let handler = signal::SigHandler::SigAction(perf_signal_handler);
    // SA_RESTART will only restart a syscall when it's safe to do so,
    // e.g. when it's a blocking read(2) or write(2). See man 7 signal.
    let flags = signal::SaFlags::SA_SIGINFO | signal::SaFlags::SA_RESTART;
    #[cfg(pprof_frame_pointer_backend)]
    let flags = if self.on_stack {
      // SA_ONSTACK will deliver the signal on an alternate stack. This is crucial
      // to prevent a stack overflow if the signal arrives at a thread with
      // a small stack, which is common when use pprof-rs in Go runtimes.
      flags | signal::SaFlags::SA_ONSTACK
    } else {
      flags
    };
    let sigaction = signal::SigAction::new(handler, flags, signal::SigSet::empty());
    let old_action = unsafe { signal::sigaction(signal::SIGPROF, &sigaction) }?;
    self.old_sigaction = Some(old_action);
    Ok(())
  }

  fn unregister_signal_handler(&mut self) -> Result<()> {
    if let Some(old_action) = self.old_sigaction.take() {
      unsafe { signal::sigaction(signal::SIGPROF, &old_action) }?;
    }
    Ok(())
  }

  // This function has to be AS-safe
  pub fn sample(
    &mut self,
    backtrace: SmallVec<[<TraceImpl as Trace>::Frame; MAX_DEPTH]>,
    thread_name: &[u8],
    thread_id: u64,
    sample_timestamp: SystemTime,
  ) {
    let frames = UnresolvedFrames::new(backtrace, thread_name, thread_id, sample_timestamp);
    self.sample_counter += 1;

    if let Ok(()) = self.data.add(frames, 1) {}
  }
}

#[cfg(test)]
pub mod tests {
  #[cfg(any(
    target_arch = "x86_64",
    target_arch = "aarch64",
    target_arch = "riscv64",
    target_arch = "loongarch64"
  ))]
  use std::ffi::OsStr;

  use strict_test_support::TestFailure;
  use strict_test_support::ensure;
  use strict_test_support::ensure_eq;
  use strict_test_support::ensure_ok;

  use super::*;

  const NO_ALLOC_PROBE_CHILD_ENV: &str = "PPROF_NO_ALLOC_PROBE_CHILD";
  struct AllocDetector {
    should_count_alloc: std::sync::atomic::AtomicBool,
    alloc_count:        std::sync::atomic::AtomicUsize,
  }

  unsafe impl std::alloc::GlobalAlloc for AllocDetector {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
      if self.should_count_alloc.load(std::sync::atomic::Ordering::SeqCst) {
        self.alloc_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
      }

      unsafe { std::alloc::System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
      unsafe { std::alloc::System.dealloc(ptr, layout) }
    }
  }
  impl AllocDetector {
    fn enable_count_alloc(&self) {
      self.should_count_alloc.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    fn disable_count_alloc(&self) {
      self.should_count_alloc.store(false, std::sync::atomic::Ordering::SeqCst);
    }

    fn reset_alloc_count(&self) {
      self.alloc_count.store(0, std::sync::atomic::Ordering::SeqCst);
    }

    fn alloc_count(&self) -> usize {
      self.alloc_count.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn count_allocations(&self) -> AllocCounterGuard<'_> {
      self.enable_count_alloc();
      AllocCounterGuard {
        alloc_detector: self
      }
    }
  }

  struct AllocCounterGuard<'a> {
    alloc_detector: &'a AllocDetector,
  }

  impl Drop for AllocCounterGuard<'_> {
    fn drop(&mut self) {
      self.alloc_detector.disable_count_alloc();
    }
  }

  #[global_allocator]
  static ALLOC: AllocDetector = AllocDetector {
    should_count_alloc: std::sync::atomic::AtomicBool::new(false),
    alloc_count:        std::sync::atomic::AtomicUsize::new(0),
  };

  struct GlobalProfilerStateRestore {
    previous: Option<Result<Profiler>>,
  }

  impl GlobalProfilerStateRestore {
    fn replace_with(state: Result<Profiler>) -> Self {
      let previous = std::mem::replace(&mut *PROFILER.write(), state);
      Self {
        previous: Some(previous)
      }
    }
  }

  impl Drop for GlobalProfilerStateRestore {
    fn drop(&mut self) {
      if let Some(previous) = self.previous.take() {
        *PROFILER.write() = previous;
      }
    }
  }

  #[cfg(any(
    target_arch = "x86_64",
    target_arch = "aarch64",
    target_arch = "riscv64",
    target_arch = "loongarch64"
  ))]
  #[derive(Clone, Debug)]
  struct TestSegment {
    name: &'static str,
    svma: usize,
    len:  usize,
  }

  #[cfg(any(
    target_arch = "x86_64",
    target_arch = "aarch64",
    target_arch = "riscv64",
    target_arch = "loongarch64"
  ))]
  impl Segment for TestSegment {
    type SharedLibrary = TestSharedLibrary;

    fn name(&self) -> &str {
      self.name
    }

    fn stated_virtual_memory_address(&self) -> findshlibs::Svma {
      findshlibs::Svma(self.svma)
    }

    fn len(&self) -> usize {
      self.len
    }
  }

  #[cfg(any(
    target_arch = "x86_64",
    target_arch = "aarch64",
    target_arch = "riscv64",
    target_arch = "loongarch64"
  ))]
  #[derive(Debug)]
  struct TestSharedLibrary {
    name:     &'static str,
    bias:     usize,
    segments: Vec<TestSegment>,
  }

  #[cfg(any(
    target_arch = "x86_64",
    target_arch = "aarch64",
    target_arch = "riscv64",
    target_arch = "loongarch64"
  ))]
  impl SharedLibrary for TestSharedLibrary {
    type Segment = TestSegment;
    type SegmentIter = std::vec::IntoIter<TestSegment>;

    fn name(&self) -> &OsStr {
      OsStr::new(self.name)
    }

    fn id(&self) -> Option<findshlibs::SharedLibraryId> {
      None
    }

    fn segments(&self) -> Self::SegmentIter {
      self.segments.clone().into_iter()
    }

    fn virtual_memory_bias(&self) -> findshlibs::Bias {
      findshlibs::Bias(self.bias)
    }

    fn each<F, C>(_f: F)
    where
      F: FnMut(&Self) -> C,
      C: Into<findshlibs::IterationControl>,
    {
    }
  }

  fn nul_terminated_thread_name_bytes(name: &[libc::c_char]) -> &[u8] {
    // Test buffers are zero-filled before the production writer stores the thread name.
    unsafe { std::ffi::CStr::from_ptr(name.as_ptr()).to_bytes() }
  }

  #[test]
  fn test_no_alloc_during_unwind() -> std::result::Result<(), TestFailure> {
    if std::env::var_os(NO_ALLOC_PROBE_CHILD_ENV).is_some() {
      return run_no_alloc_during_unwind_probe();
    }

    let test_binary = ensure_ok(std::env::current_exe(), "current test binary should be discoverable")?;
    let status = ensure_ok(
      std::process::Command::new(test_binary)
        .arg("--exact")
        .arg("profiler::tests::test_no_alloc_during_unwind")
        .arg("--nocapture")
        .env(NO_ALLOC_PROBE_CHILD_ENV, "1")
        .status(),
      "allocation probe child should run",
    )?;

    ensure(status.success(), "allocation probe child should pass")
  }

  #[cfg(any(
    target_arch = "x86_64",
    target_arch = "aarch64",
    target_arch = "riscv64",
    target_arch = "loongarch64"
  ))]
  #[test]
  fn address_strictly_inside_segment_is_blocklisted() -> std::result::Result<(), TestFailure> {
    ensure(
      address_in_segments(15, &[(10, 20)]),
      "address inside open interval should be blocked",
    )
  }

  #[cfg(any(
    target_arch = "x86_64",
    target_arch = "aarch64",
    target_arch = "riscv64",
    target_arch = "loongarch64"
  ))]
  #[test]
  fn address_on_segment_boundary_is_not_blocklisted() -> std::result::Result<(), TestFailure> {
    ensure(
      !address_in_segments(10, &[(10, 20)]),
      "segment start boundary should not be blocked",
    )?;
    ensure(!address_in_segments(20, &[(10, 20)]), "segment end boundary should not be blocked")
  }

  #[cfg(any(
    target_arch = "x86_64",
    target_arch = "aarch64",
    target_arch = "riscv64",
    target_arch = "loongarch64"
  ))]
  #[test]
  fn address_outside_or_without_segments_is_not_blocklisted() -> std::result::Result<(), TestFailure> {
    ensure(
      !address_in_segments(5, &[(10, 20)]),
      "address before a segment should not be blocked",
    )?;
    ensure(
      !address_in_segments(25, &[(10, 20)]),
      "address after a segment should not be blocked",
    )?;
    ensure(
      !address_in_segments(15, &[]),
      "address without configured segments should not be blocked",
    )
  }

  #[cfg(any(
    target_arch = "x86_64",
    target_arch = "aarch64",
    target_arch = "riscv64",
    target_arch = "loongarch64"
  ))]
  #[test]
  fn library_name_matching_respects_blocklist_and_missing_names() -> std::result::Result<(), TestFailure> {
    let blocklist = ["pthread", "vdso"];

    ensure(
      library_name_matches_blocklist(Some("libpthread.so"), &blocklist),
      "matching library name should be blocklisted",
    )?;
    ensure(
      !library_name_matches_blocklist(Some("libm.so"), &blocklist),
      "non-matching library name should not be blocklisted",
    )?;
    ensure(
      !library_name_matches_blocklist(None, &blocklist),
      "non-utf8 or missing library name should be ignored",
    )
  }

  #[cfg(any(
    target_arch = "x86_64",
    target_arch = "aarch64",
    target_arch = "riscv64",
    target_arch = "loongarch64"
  ))]
  #[test]
  fn blocklist_segments_default_is_empty() -> std::result::Result<(), TestFailure> {
    let segments = BlocklistSegments::default();

    ensure(!segments.contains(15), "default blocklist segments should not block addresses")
  }

  #[cfg(any(
    target_arch = "x86_64",
    target_arch = "aarch64",
    target_arch = "riscv64",
    target_arch = "loongarch64"
  ))]
  #[test]
  fn blocklisted_library_segments_are_appended_only_for_matching_libraries() -> std::result::Result<(), TestFailure> {
    let blocklist = ["blocked"];
    let mut blocklist_segments = Vec::new();
    let unmatched_library = TestSharedLibrary {
      name:     "/usr/lib/liballowed.so",
      bias:     0x1000,
      segments: vec![TestSegment {
        name: "text",
        svma: 0x100,
        len:  0x20,
      }],
    };
    let matched_library = TestSharedLibrary {
      name:     "/usr/lib/libblocked.so",
      bias:     0x1000,
      segments: vec![
        TestSegment {
          name: "text",
          svma: 0x100,
          len:  0x20,
        },
        TestSegment {
          name: "data",
          svma: 0x300,
          len:  0x40,
        },
      ],
    };

    append_blocklisted_library_segments(&unmatched_library, &blocklist, &mut blocklist_segments);
    ensure(
      blocklist_segments.is_empty(),
      "non-matching shared libraries should not append blocked segments",
    )?;

    append_blocklisted_library_segments(&matched_library, &blocklist, &mut blocklist_segments);
    ensure(
      blocklist_segments == vec![(0x1100, 0x1120), (0x1300, 0x1340)],
      "matching shared libraries should append biased segment ranges",
    )
  }

  #[cfg(any(
    target_arch = "x86_64",
    target_arch = "aarch64",
    target_arch = "riscv64",
    target_arch = "loongarch64"
  ))]
  #[test]
  fn profiler_builder_ignores_unmatched_blocklist_entries() -> std::result::Result<(), TestFailure> {
    let builder = ProfilerGuardBuilder::default().blocklist(&["definitely-not-a-loaded-library"]);

    ensure(
      builder.blocklist_segments.ranges.is_empty(),
      "unmatched blocklist entries should not collect blocked segments",
    )
  }

  #[test]
  fn profiler_guard_builder_default_keeps_profiler_configuration_empty() -> std::result::Result<(), TestFailure> {
    let builder = ProfilerGuardBuilder::default();

    ensure_eq(
      &builder.frequency,
      &99,
      "default profiler frequency should match legacy sampling rate",
    )?;
    #[cfg(any(
      target_arch = "x86_64",
      target_arch = "aarch64",
      target_arch = "riscv64",
      target_arch = "loongarch64"
    ))]
    ensure(
      builder.blocklist_segments.ranges.is_empty(),
      "default profiler builder should not block any loaded-library segments",
    )?;
    #[cfg(pprof_frame_pointer_backend)]
    ensure(
      !builder.on_stack,
      "default frame-pointer profiler should not use alternate signal stack",
    )?;
    Ok(())
  }

  #[cfg(any(
    target_arch = "x86_64",
    target_arch = "aarch64",
    target_arch = "riscv64",
    target_arch = "loongarch64"
  ))]
  #[test]
  fn profiler_guard_builder_frequency_preserves_blocklist_segments() -> std::result::Result<(), TestFailure> {
    let builder = ProfilerGuardBuilder {
      blocklist_segments: BlocklistSegments {
        ranges: vec![(10, 20)]
      },
      ..ProfilerGuardBuilder::default()
    };

    let configured = builder.frequency(250);

    ensure_eq(&configured.frequency, &250, "frequency setter should replace only frequency")?;
    ensure(
      configured.blocklist_segments.contains(15),
      "frequency setter should preserve collected blocklist segments",
    )
  }

  #[test]
  fn profiler_guard_builder_configures_report_frequency() -> std::result::Result<(), TestFailure> {
    let _guard = PROFILER_SIGNAL_TEST_LOCK.lock();
    let profiler = ensure_ok(
      ProfilerGuardBuilder::default().frequency(250).build(),
      "profiler guard should start with configured frequency",
    )?;
    let report = ensure_ok(
      profiler.report().build_unresolved(),
      "profiler guard should build unresolved report",
    )?;

    ensure_eq(&report.timing.frequency, &250, "report timing should use configured frequency")
  }

  #[test]
  fn profiler_guard_builder_rejects_non_positive_frequency() -> std::result::Result<(), TestFailure> {
    for frequency in [0, -1] {
      ensure(
        ProfilerGuardBuilder::default()
          .frequency(frequency)
          .build()
          .err()
          .map(|err| err.to_string())
          == Some(format!(
            "invalid profiler frequency {frequency}; expected a positive sampling frequency"
          )),
        "profiler guard builder should reject non-positive frequencies before starting",
      )?;
    }
    Ok(())
  }

  #[test]
  fn profiler_guard_builder_returns_initialization_error_when_global_profiler_state_is_error() -> std::result::Result<(), TestFailure> {
    let _guard = PROFILER_SIGNAL_TEST_LOCK.lock();
    let _restore = GlobalProfilerStateRestore::replace_with(Err(Error::ProfilerInitialization));

    ensure(
      ProfilerGuardBuilder::default().build().err().map(|err| err.to_string()) == Some("profiler failed to initialize".to_owned()),
      "profiler guard builder should map stored global profiler errors to initialization failure",
    )
  }

  #[test]
  fn profiler_guard_report_without_timer_uses_default_timing() -> std::result::Result<(), TestFailure> {
    let profiler_state = RwLock::new(Profiler::new());
    let profiler_guard = ProfilerGuard {
      profiler: &profiler_state,
      timer:    None,
    };

    let report = ensure_ok(
      profiler_guard.report().build_unresolved(),
      "profiler guard without timer should still build unresolved report",
    )?;

    ensure_eq(
      &report.timing.frequency,
      &1,
      "profiler guard without timer should use default report frequency",
    )?;
    ensure(
      report.timing.start_time == SystemTime::UNIX_EPOCH,
      "profiler guard without timer should use default report start time",
    )?;
    ensure(
      report.timing.duration == Default::default(),
      "profiler guard without timer should use default report duration",
    )
  }

  #[test]
  fn profiler_new_initializes_stopped_empty_state() -> std::result::Result<(), TestFailure> {
    let profiler = ensure_ok(Profiler::new(), "profiler should initialize")?;

    ensure_eq(&profiler.sample_counter, &0, "new profiler should reset sample counter")?;
    ensure(!profiler.running, "new profiler should not be running")?;
    ensure(
      profiler.old_sigaction.is_none(),
      "new profiler should not have registered signal state",
    )
  }

  #[cfg(any(
    target_arch = "x86_64",
    target_arch = "aarch64",
    target_arch = "riscv64",
    target_arch = "loongarch64"
  ))]
  #[test]
  fn profiler_uses_blocklist_segments_for_membership() -> std::result::Result<(), TestFailure> {
    let mut profiler = ensure_ok(Profiler::new(), "profiler should initialize")?;
    profiler.blocklist_segments = BlocklistSegments {
      ranges: vec![(10, 20)]
    };

    ensure(profiler.is_blocklisted(15), "address inside blocklist segment should be blocked")?;
    ensure(!profiler.is_blocklisted(10), "profiler blocklist should exclude the start boundary")?;
    ensure(!profiler.is_blocklisted(20), "profiler blocklist should exclude the end boundary")
  }

  #[test]
  fn unregister_signal_handler_without_saved_action_is_noop() -> std::result::Result<(), TestFailure> {
    let mut profiler = ensure_ok(Profiler::new(), "profiler should initialize")?;

    ensure_ok(
      profiler.unregister_signal_handler(),
      "unregistering without saved signal action should be a no-op",
    )?;
    ensure(
      profiler.old_sigaction.is_none(),
      "no-op signal unregister should leave saved signal action empty",
    )
  }

  #[test]
  fn profiler_guard_drop_tolerates_error_state() -> std::result::Result<(), TestFailure> {
    let profiler_state = RwLock::new(Err(Error::ProfilerInitialization));
    {
      let _profiler_guard = ProfilerGuard {
        profiler: &profiler_state,
        timer:    None,
      };
    }

    ensure(
      profiler_state.read().as_ref().err().map(ToString::to_string) == Some("profiler failed to initialize".to_owned()),
      "dropping a profiler guard should tolerate an error profiler state",
    )
  }

  #[test]
  fn profiler_guard_drop_tolerates_stopped_profiler() -> std::result::Result<(), TestFailure> {
    let profiler_state = RwLock::new(Profiler::new());
    {
      let _profiler_guard = ProfilerGuard {
        profiler: &profiler_state,
        timer:    None,
      };
    }

    match profiler_state.read().as_ref() {
      Ok(profiler) => ensure(
        !profiler.running,
        "dropping a guard over an already stopped profiler should leave it stopped",
      ),
      Err(_) => ensure(
        false,
        "dropping a guard over a stopped profiler should not replace it with an error",
      ),
    }
  }

  #[cfg(any(
    target_os = "android",
    target_os = "linux",
    target_os = "macos",
    target_os = "freebsd"
  ))]
  #[test]
  fn errno_protector_restores_original_errno() -> std::result::Result<(), TestFailure> {
    let _restore_original_errno = ErrnoProtector::new();
    set_current_errno(17);

    {
      let _protector = ErrnoProtector::new();
      set_current_errno(29);
      ensure_eq(
        &current_errno(),
        &29,
        "errno changes inside the protected region should remain visible until drop",
      )?;
    }

    ensure_eq(
      &current_errno(),
      &17,
      "errno protector should restore the value captured at construction",
    )
  }

  #[cfg(any(
    target_os = "android",
    target_os = "linux",
    target_os = "macos",
    target_os = "freebsd"
  ))]
  #[test]
  fn signal_handler_preserves_errno_when_profiler_lock_is_busy() -> std::result::Result<(), TestFailure> {
    let _guard = PROFILER_SIGNAL_TEST_LOCK.lock();
    let _restore_original_errno = ErrnoProtector::new();
    let _profiler_lock = PROFILER.write();
    set_current_errno(31);

    perf_signal_handler(0, std::ptr::null_mut(), std::ptr::null_mut());

    ensure_eq(
      &current_errno(),
      &31,
      "signal handler should preserve errno when profiler lock is busy",
    )
  }

  #[cfg(any(
    target_os = "android",
    target_os = "linux",
    target_os = "macos",
    target_os = "freebsd"
  ))]
  #[test]
  fn signal_handler_preserves_errno_when_profiler_state_is_error() -> std::result::Result<(), TestFailure> {
    let _guard = PROFILER_SIGNAL_TEST_LOCK.lock();
    let _restore_profiler = GlobalProfilerStateRestore::replace_with(Err(Error::ProfilerInitialization));
    let _restore_original_errno = ErrnoProtector::new();
    set_current_errno(37);

    perf_signal_handler(0, std::ptr::null_mut(), std::ptr::null_mut());

    ensure_eq(
      &current_errno(),
      &37,
      "signal handler should preserve errno when profiler state is an error",
    )
  }

  #[test]
  fn profiler_sample_records_entry_and_init_resets_state() -> std::result::Result<(), TestFailure> {
    let mut profiler = ensure_ok(Profiler::new(), "profiler should initialize")?;

    profiler.sample(SmallVec::new(), b"sample-thread", 77, SystemTime::UNIX_EPOCH);

    ensure_eq(&profiler.sample_counter, &1, "sample should increment counter")?;
    let entries = ensure_ok(profiler.data.try_iter(), "sample entries should iterate")?;
    ensure_eq(&entries.count(), &1, "sample should add one collector entry")?;

    ensure_ok(profiler.init(), "profiler init should reset state")?;

    ensure_eq(&profiler.sample_counter, &0, "init should reset sample counter")?;
    ensure(!profiler.running, "init should leave profiler stopped")?;
    ensure_eq(
      &ensure_ok(profiler.data.try_iter(), "reset sample entries should iterate")?.count(),
      &0,
      "init should reset collector data",
    )
  }

  #[test]
  fn profiler_start_and_stop_manage_signal_lifecycle_and_invalid_transitions() -> std::result::Result<(), TestFailure> {
    let _guard = PROFILER_SIGNAL_TEST_LOCK.lock();
    let mut profiler = ensure_ok(Profiler::new(), "profiler should initialize")?;

    ensure_ok(profiler.start(), "profiler should start")?;
    ensure(profiler.running, "started profiler should be running")?;
    ensure(
      profiler.old_sigaction.is_some(),
      "started profiler should save the previous signal action",
    )?;

    ensure(
      profiler.start().err().map(|err| err.to_string()) == Some("profiler is already running".to_owned()),
      "starting a running profiler should fail",
    )?;

    ensure_ok(profiler.stop(), "profiler should stop")?;
    ensure(!profiler.running, "stopped profiler should not be running")?;
    ensure(
      profiler.old_sigaction.is_none(),
      "stopped profiler should restore and clear saved signal action",
    )?;
    ensure(
      profiler.stop().err().map(|err| err.to_string()) == Some("profiler is not running".to_owned()),
      "stopping a stopped profiler should fail",
    )
  }

  #[test]
  fn profiler_guard_rejects_concurrent_start_and_restarts_after_drop() -> std::result::Result<(), TestFailure> {
    let _guard = PROFILER_SIGNAL_TEST_LOCK.lock();
    let first_profiler = ensure_ok(
      ProfilerGuardBuilder::default().frequency(1).build(),
      "first profiler guard should start",
    )?;

    ensure(
      ProfilerGuardBuilder::default()
        .frequency(1)
        .build()
        .err()
        .map(|err| err.to_string())
        == Some("profiler is already running".to_owned()),
      "second profiler guard should be rejected while the global profiler is running",
    )?;

    drop(first_profiler);

    let restarted_profiler = ensure_ok(
      ProfilerGuardBuilder::default().frequency(1).build(),
      "profiler guard should restart after the first guard drops",
    )?;
    drop(restarted_profiler);
    Ok(())
  }

  #[cfg(any(
    target_arch = "x86_64",
    target_arch = "aarch64",
    target_arch = "riscv64",
    target_arch = "loongarch64"
  ))]
  #[test]
  fn null_signal_context_has_no_instruction_pointer() -> std::result::Result<(), TestFailure> {
    ensure(
      signal_context_instruction_pointer(std::ptr::null_mut()).is_none(),
      "null signal context should not produce an instruction pointer",
    )
  }

  #[cfg(any(
    target_arch = "x86_64",
    target_arch = "aarch64",
    target_arch = "riscv64",
    target_arch = "loongarch64"
  ))]
  #[test]
  fn null_signal_context_is_not_blocklisted() -> std::result::Result<(), TestFailure> {
    let mut profiler = ensure_ok(Profiler::new(), "profiler should initialize")?;
    profiler.blocklist_segments = BlocklistSegments {
      ranges: vec![(1, usize::MAX)],
    };

    ensure(
      !signal_context_is_blocklisted(&profiler, std::ptr::null_mut()),
      "null signal context should not be treated as a blocked sampled address",
    )
  }

  #[cfg(all(target_arch = "x86_64", target_os = "linux"))]
  fn linux_signal_context_with_instruction_pointer(ip: usize) -> std::result::Result<libc::ucontext_t, TestFailure> {
    let mut context: libc::ucontext_t = unsafe { std::mem::zeroed() };
    let register_index = ensure_ok(
      usize::try_from(libc::REG_RIP),
      "linux instruction-pointer register index should fit usize",
    )?;
    context.uc_mcontext.gregs[register_index] = ensure_ok(ip.try_into(), "linux instruction-pointer test address should fit greg_t")?;
    Ok(context)
  }

  #[cfg(all(target_arch = "x86_64", target_os = "linux"))]
  #[test]
  fn linux_signal_context_exposes_instruction_pointer() -> std::result::Result<(), TestFailure> {
    let mut context = linux_signal_context_with_instruction_pointer(0x1234)?;

    ensure(
      signal_context_instruction_pointer(std::ptr::addr_of_mut!(context).cast::<libc::c_void>()) == Some(0x1234),
      "linux signal context should expose RIP as instruction pointer",
    )
  }

  #[cfg(all(target_arch = "x86_64", target_os = "linux"))]
  #[test]
  fn linux_signal_context_uses_instruction_pointer_for_blocklist() -> std::result::Result<(), TestFailure> {
    let mut profiler = ensure_ok(Profiler::new(), "profiler should initialize")?;
    profiler.blocklist_segments = BlocklistSegments {
      ranges: vec![(0x1000, 0x2000)],
    };
    let mut context = linux_signal_context_with_instruction_pointer(0x1234)?;

    ensure(
      signal_context_is_blocklisted(&profiler, std::ptr::addr_of_mut!(context).cast::<libc::c_void>()),
      "signal context instruction pointer inside blocked segment should be blocked",
    )
  }

  #[cfg(all(target_arch = "x86_64", target_os = "linux"))]
  #[test]
  fn signal_handler_skips_blocklisted_context_without_sampling() -> std::result::Result<(), TestFailure> {
    let _guard = PROFILER_SIGNAL_TEST_LOCK.lock();
    let mut profiler = ensure_ok(Profiler::new(), "profiler should initialize")?;
    profiler.blocklist_segments = BlocklistSegments {
      ranges: vec![(0x1000, 0x2000)],
    };
    let _restore = GlobalProfilerStateRestore::replace_with(Ok(profiler));
    let mut context = linux_signal_context_with_instruction_pointer(0x1234)?;

    perf_signal_handler(0, std::ptr::null_mut(), std::ptr::addr_of_mut!(context).cast::<libc::c_void>());

    let sample_counter = PROFILER
      .read()
      .as_ref()
      .map(|profiler| profiler.sample_counter)
      .map_err(ToString::to_string);
    ensure(
      sample_counter == Ok(0),
      "signal handler should skip blocklisted contexts before recording a sample",
    )
  }

  #[cfg(all(target_arch = "x86_64", target_os = "linux"))]
  #[test]
  fn linux_signal_context_outside_blocklist_is_not_blocklisted() -> std::result::Result<(), TestFailure> {
    let mut profiler = ensure_ok(Profiler::new(), "profiler should initialize")?;
    profiler.blocklist_segments = BlocklistSegments {
      ranges: vec![(0x1000, 0x2000)],
    };
    let mut context = linux_signal_context_with_instruction_pointer(0x3000)?;

    ensure(
      !signal_context_is_blocklisted(&profiler, std::ptr::addr_of_mut!(context).cast::<libc::c_void>()),
      "signal context instruction pointer outside blocked segments should not be blocked",
    )
  }

  #[test]
  fn frame_collection_stops_at_max_depth() -> std::result::Result<(), TestFailure> {
    let mut frames = SmallVec::<[usize; MAX_DEPTH]>::with_capacity(MAX_DEPTH);

    for frame in 0..MAX_DEPTH {
      ensure(
        push_frame_within_max_depth(&mut frames, &frame),
        "frame collection should accept frames until the max depth is reached",
      )?;
    }

    let overflowing_frame = MAX_DEPTH;
    ensure_eq(&frames.len(), &MAX_DEPTH, "frame collection should reach max depth")?;
    ensure(
      !push_frame_within_max_depth(&mut frames, &overflowing_frame),
      "frame collection should stop when max depth is reached",
    )?;
    ensure_eq(
      &frames.len(),
      &MAX_DEPTH,
      "rejected frames should not change collected frame length",
    )
  }

  #[test]
  fn thread_name_fallback_writes_numeric_thread_id() -> std::result::Result<(), TestFailure> {
    let mut name = [0; MAX_THREAD_NAME];

    write_thread_name_fallback(12345, &mut name);

    ensure(
      name[..5] == [49_i8, 50_i8, 51_i8, 52_i8, 53_i8],
      "fallback thread name should contain decimal thread id",
    )
  }

  #[cfg(any(target_os = "linux", target_os = "android"))]
  #[test]
  fn thread_name_fallback_caps_large_thread_ids_at_buffer_width() -> std::result::Result<(), TestFailure> {
    let large_thread_id = ensure_ok(
      libc::pthread_t::try_from(10_000_000_000_000_000_u128),
      "large thread id should fit pthread_t on linux-like targets",
    )?;
    let mut name = [0; MAX_THREAD_NAME];

    write_thread_name_fallback(large_thread_id, &mut name);

    ensure(
      name
        == [
          49_i8, 48_i8, 48_i8, 48_i8, 48_i8, 48_i8, 48_i8, 48_i8, 48_i8, 48_i8, 48_i8, 48_i8, 48_i8, 48_i8, 48_i8, 48_i8,
        ],
      "fallback thread name should fill the fixed-width buffer for very large thread ids",
    )
  }

  #[test]
  fn thread_name_fallback_handles_single_digit_and_zero_thread_ids() -> std::result::Result<(), TestFailure> {
    let mut single_digit_name = [0; MAX_THREAD_NAME];
    let mut power_of_ten_name = [0; MAX_THREAD_NAME];
    let mut zero_name = [0; MAX_THREAD_NAME];

    write_thread_name_fallback(7, &mut single_digit_name);
    write_thread_name_fallback(10, &mut power_of_ten_name);
    write_thread_name_fallback(0, &mut zero_name);

    ensure_eq(
      &single_digit_name[0],
      &55_i8,
      "single digit fallback thread name should write the ASCII digit",
    )?;
    ensure(
      power_of_ten_name[..2] == [49_i8, 48_i8],
      "power-of-ten fallback thread name should preserve every decimal digit",
    )?;
    ensure_eq(&zero_name[0], &0, "zero fallback thread name should remain an empty C string")
  }

  #[cfg(all(any(target_os = "linux", target_os = "macos"), target_env = "gnu"))]
  #[test]
  fn write_thread_name_uses_fallback_when_native_lookup_fails() -> std::result::Result<(), TestFailure> {
    let mut name = [0; MAX_THREAD_NAME];

    write_thread_name_after_native_lookup(12345, &mut name, 1);

    ensure(
      nul_terminated_thread_name_bytes(&name) == b"12345",
      "failed native thread-name lookup should write fallback numeric thread id",
    )
  }

  #[cfg(all(any(target_os = "linux", target_os = "macos"), target_env = "gnu"))]
  #[test]
  fn write_thread_name_preserves_native_name_when_lookup_succeeds() -> std::result::Result<(), TestFailure> {
    let mut name = [0; MAX_THREAD_NAME];
    let native_name = b"native-name";
    for (name_byte, source_byte) in name.iter_mut().zip(native_name.iter().copied()) {
      *name_byte = ensure_ok(libc::c_char::try_from(source_byte), "native thread-name fixture should fit c_char")?;
    }

    write_thread_name_after_native_lookup(12345, &mut name, 0);

    ensure(
      nul_terminated_thread_name_bytes(&name) == native_name,
      "successful native thread-name lookup should preserve the native name",
    )
  }

  #[cfg(all(target_os = "linux", target_env = "gnu"))]
  #[test]
  fn write_thread_name_reads_native_name_for_current_thread() -> std::result::Result<(), TestFailure> {
    let handle = ensure_ok(
      std::thread::Builder::new().name("pprof-native".to_owned()).spawn(|| {
        let mut name = [0; MAX_THREAD_NAME];
        let current_thread = unsafe { libc::pthread_self() };
        write_thread_name(current_thread, &mut name);
        name
      }),
      "named thread should spawn",
    )?;

    let name = match handle.join() {
      Ok(name) => name,
      Err(_) => {
        return ensure(false, "named thread should join successfully");
      }
    };

    ensure(
      nul_terminated_thread_name_bytes(&name) == b"pprof-native",
      "native thread-name lookup should read the current thread name",
    )
  }

  fn run_no_alloc_during_unwind_probe() -> std::result::Result<(), TestFailure> {
    trigger_lazy();

    ALLOC.reset_alloc_count();
    let _ignored_alloc = Box::new(1usize);
    ensure_eq(&ALLOC.alloc_count(), &0, "allocation count should ignore disabled allocations")?;

    ALLOC.reset_alloc_count();
    {
      let _alloc_counter_guard = ALLOC.count_allocations();
      let _counted_alloc = Box::new(1usize);
    }
    ensure_eq(&ALLOC.alloc_count(), &1, "allocation count should track enabled allocation")?;
    ALLOC.reset_alloc_count();

    let _guard = ensure_ok(ProfilerGuard::new(999), "profiler should start for allocation probe")?;
    let start = std::time::Instant::now();
    {
      let _alloc_counter_guard = ALLOC.count_allocations();
      let _alloc = Box::new(1usize);
      // busy loop for a while to trigger some samples
      while start.elapsed().as_millis() < 500 {
        std::hint::black_box(());
      }
    }
    ensure_eq(
      &ALLOC.alloc_count(),
      &1,
      "unwinding should not allocate beyond the measured allocation",
    )
  }
}
