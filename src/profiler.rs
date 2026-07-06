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
use crate::report::ReportBuilder;
use crate::timer::Timer;

pub(crate) static PROFILER: Lazy<RwLock<Result<Profiler>>> = Lazy::new(|| RwLock::new(Profiler::new()));

pub struct Profiler {
  pub(crate) data: Collector<UnresolvedFrames>,
  sample_counter:  i32,

  old_sigaction: Option<signal::SigAction>,
  running:       bool,

  #[cfg(pprof_frame_pointer_backend)]
  on_stack: bool,

  #[cfg(any(
    target_arch = "x86_64",
    target_arch = "aarch64",
    target_arch = "riscv64",
    target_arch = "loongarch64"
  ))]
  blocklist_segments: Vec<(usize, usize)>,
}

#[derive(Clone)]
pub struct ProfilerGuardBuilder {
  frequency: c_int,

  #[cfg(pprof_frame_pointer_backend)]
  on_stack: bool,

  #[cfg(any(
    target_arch = "x86_64",
    target_arch = "aarch64",
    target_arch = "riscv64",
    target_arch = "loongarch64"
  ))]
  blocklist_segments: Vec<(usize, usize)>,
}

impl Default for ProfilerGuardBuilder {
  fn default() -> ProfilerGuardBuilder {
    ProfilerGuardBuilder {
      frequency: 99,

      #[cfg(pprof_frame_pointer_backend)]
      on_stack:                                     false,

      #[cfg(any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "riscv64",
        target_arch = "loongarch64"
      ))]
      blocklist_segments: Vec::new(),
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
    let blocklist_segments = {
      let mut segments = Vec::new();
      TargetSharedLibrary::each(|shlib| {
        let in_blocklist = match shlib.name().to_str() {
          Some(name) => {
            let mut in_blocklist = false;
            for blocked_name in blocklist.iter() {
              if name.contains(blocked_name.as_ref()) {
                in_blocklist = true;
              }
            }

            in_blocklist
          }

          None => false,
        };
        if in_blocklist {
          for seg in shlib.segments() {
            let avam = seg.actual_virtual_memory_address(shlib);
            let start = avam.0;
            let end = start + seg.len();
            segments.push((start, end));
          }
        }
      });
      segments
    };

    Self {
      blocklist_segments,
      ..self
    }
  }
  pub fn build(self) -> Result<ProfilerGuard<'static>> {
    trigger_lazy();

    match PROFILER.write().as_mut() {
      Err(err) => {
        log::error!("Error in creating profiler: {}", err);
        Err(Error::CreatingError)
      }
      Ok(profiler) => {
        #[cfg(pprof_frame_pointer_backend)]
        {
          profiler.on_stack = self.on_stack;
        }

        #[cfg(any(
          target_arch = "x86_64",
          target_arch = "aarch64",
          target_arch = "riscv64",
          target_arch = "loongarch64"
        ))]
        {
          profiler.blocklist_segments = self.blocklist_segments;
        }

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
  let mut len = 0;
  let mut base = 1;

  while current_thread as u128 > base && len < MAX_THREAD_NAME {
    base *= 10;
    len += 1;
  }

  let mut index = 0;
  while index < len && base > 1 {
    base /= 10;

    name[index] = match (48 + (current_thread as u128 / base) % 10).try_into() {
      Ok(digit) => digit,
      Err(_) => {
        log::error!("fail to convert thread_id to string");
        0
      }
    };

    index += 1;
  }
}

#[cfg(not(all(any(target_os = "linux", target_os = "macos"), target_env = "gnu")))]
fn write_thread_name(current_thread: libc::pthread_t, name: &mut [libc::c_char]) {
  write_thread_name_fallback(current_thread, name);
}

#[cfg(all(any(target_os = "linux", target_os = "macos"), target_env = "gnu"))]
fn write_thread_name(current_thread: libc::pthread_t, name: &mut [libc::c_char]) {
  let name_ptr = name as *mut [libc::c_char] as *mut libc::c_char;
  let ret = unsafe { libc::pthread_getname_np(current_thread, name_ptr, MAX_THREAD_NAME) };

  if ret != 0 {
    write_thread_name_fallback(current_thread, name);
  }
}

struct ErrnoProtector(libc::c_int);

impl ErrnoProtector {
  fn new() -> Self {
    unsafe {
      #[cfg(target_os = "android")]
      {
        let errno = *libc::__errno();
        Self(errno)
      }
      #[cfg(target_os = "linux")]
      {
        let errno = *libc::__errno_location();
        Self(errno)
      }
      #[cfg(any(target_os = "macos", target_os = "freebsd"))]
      {
        let errno = *libc::__error();
        Self(errno)
      }
    }
  }
}

impl Drop for ErrnoProtector {
  fn drop(&mut self) {
    unsafe {
      #[cfg(target_os = "android")]
      {
        *libc::__errno() = self.0;
      }
      #[cfg(target_os = "linux")]
      {
        *libc::__errno_location() = self.0;
      }
      #[cfg(any(target_os = "macos", target_os = "freebsd"))]
      {
        *libc::__error() = self.0;
      }
    }
  }
}

#[unsafe(no_mangle)]
#[cfg_attr(
  not(all(any(
    target_arch = "x86_64",
    target_arch = "aarch64",
    target_arch = "riscv64",
    target_arch = "loongarch64"
  ))),
  allow(unused_variables)
)]
#[allow(clippy::unnecessary_cast)]
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
  if !ucontext.is_null() {
    let ucontext: *mut libc::ucontext_t = ucontext as *mut libc::ucontext_t;

    #[cfg(all(target_arch = "x86_64", target_os = "linux"))]
    let addr = unsafe { (*ucontext).uc_mcontext.gregs[libc::REG_RIP as usize] as usize };

    #[cfg(all(target_arch = "x86_64", target_os = "freebsd"))]
    let addr = unsafe { (*ucontext).uc_mcontext.mc_rip as usize };

    #[cfg(all(target_arch = "x86_64", target_os = "macos"))]
    let addr = unsafe {
      let mcontext = (*ucontext).uc_mcontext;
      if mcontext.is_null() {
        0
      } else {
        (*mcontext).__ss.__rip as usize
      }
    };

    #[cfg(all(target_arch = "aarch64", any(target_os = "android", target_os = "linux")))]
    let addr = unsafe { (*ucontext).uc_mcontext.pc as usize };

    #[cfg(all(target_arch = "aarch64", target_os = "freebsd"))]
    let addr = unsafe { (*ucontext).mc_gpregs.gp_elr as usize };

    #[cfg(all(target_arch = "aarch64", target_os = "macos"))]
    let addr = unsafe {
      let mcontext = (*ucontext).uc_mcontext;
      if mcontext.is_null() {
        0
      } else {
        (*mcontext).__ss.__pc as usize
      }
    };

    #[cfg(all(target_arch = "riscv64", target_os = "linux"))]
    let addr = unsafe { (*ucontext).uc_mcontext.__gregs[libc::REG_PC] as usize };

    #[cfg(all(target_arch = "loongarch64", target_os = "linux"))]
    let addr = unsafe { (*ucontext).uc_mcontext.__pc as usize };

    if profiler.is_blocklisted(addr) {
      return;
    }
  }

  let mut bt: SmallVec<[<TraceImpl as Trace>::Frame; MAX_DEPTH]> = SmallVec::with_capacity(MAX_DEPTH);
  let mut index = 0;

  let sample_timestamp: SystemTime = SystemTime::now();
  TraceImpl::trace(ucontext, |frame| {
    #[cfg(pprof_frame_pointer_backend)]
    {
      let ip = crate::backtrace::Frame::ip(frame);
      if profiler.is_blocklisted(ip) {
        return false;
      }
    }

    if index < MAX_DEPTH {
      bt.push(frame.clone());
      index += 1;
      true
    } else {
      false
    }
  });

  let current_thread = unsafe { libc::pthread_self() };
  let mut name = [0; MAX_THREAD_NAME];
  let name_ptr = &mut name as *mut [libc::c_char] as *mut libc::c_char;

  write_thread_name(current_thread, &mut name);

  let name = unsafe { std::ffi::CStr::from_ptr(name_ptr) };
  profiler.sample(bt, name.to_bytes(), current_thread as u64, sample_timestamp);
}

impl Profiler {
  fn new() -> Result<Self> {
    Ok(Profiler {
      data:           Collector::new()?,
      sample_counter: 0,
      old_sigaction:  None,
      running:        false,

      #[cfg(pprof_frame_pointer_backend)]
      on_stack:                                     false,

      #[cfg(any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "riscv64",
        target_arch = "loongarch64"
      ))]
      blocklist_segments: Vec::new(),
    })
  }

  #[cfg(any(
    target_arch = "x86_64",
    target_arch = "aarch64",
    target_arch = "riscv64",
    target_arch = "loongarch64"
  ))]
  fn is_blocklisted(&self, addr: usize) -> bool {
    for libs in &self.blocklist_segments {
      if addr > libs.0 && addr < libs.1 {
        return true;
      }
    }
    false
  }
}

impl Profiler {
  pub fn start(&mut self) -> Result<()> {
    log::info!("starting cpu profiler");
    if self.running {
      Err(Error::Running)
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

  #[test]
  fn test_no_alloc_during_unwind() {
    if std::env::var_os(NO_ALLOC_PROBE_CHILD_ENV).is_some() {
      run_no_alloc_during_unwind_probe();
      return;
    }

    let test_binary = std::env::current_exe().unwrap();
    let status = std::process::Command::new(test_binary)
      .arg("--exact")
      .arg("profiler::tests::test_no_alloc_during_unwind")
      .arg("--nocapture")
      .env(NO_ALLOC_PROBE_CHILD_ENV, "1")
      .status()
      .unwrap();

    assert!(status.success());
  }

  fn run_no_alloc_during_unwind_probe() {
    trigger_lazy();

    ALLOC.reset_alloc_count();
    let _ignored_alloc = Box::new(1usize);
    assert_eq!(ALLOC.alloc_count(), 0);

    ALLOC.reset_alloc_count();
    {
      let _alloc_counter_guard = ALLOC.count_allocations();
      let _counted_alloc = Box::new(1usize);
    }
    assert_eq!(ALLOC.alloc_count(), 1);
    ALLOC.reset_alloc_count();

    let _guard = ProfilerGuard::new(999).unwrap();
    let start = std::time::Instant::now();
    {
      let _alloc_counter_guard = ALLOC.count_allocations();
      let _alloc = Box::new(1usize);
      // busy loop for a while to trigger some samples
      while start.elapsed().as_millis() < 500 {
        std::hint::black_box(());
      }
    }
    assert_eq!(ALLOC.alloc_count(), 1);
  }
}
