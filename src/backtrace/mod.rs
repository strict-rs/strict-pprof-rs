// Copyright 2022 TiKV Project Authors. Licensed under Apache-2.0.

use std::path::PathBuf;

use libc::c_void;

#[cfg(all(
  any(pprof_framehop_backend, pprof_frame_pointer_backend),
  not(any(target_os = "macos", target_os = "ios"))
))]
unsafe extern "C" {
  fn _Unwind_FindEnclosingFunction(pc: *mut c_void) -> *mut c_void;
}

#[cfg(any(pprof_framehop_backend, pprof_frame_pointer_backend))]
pub(crate) fn symbol_address_from_ip(ip: usize) -> *mut c_void {
  if cfg!(target_os = "macos") || cfg!(target_os = "ios") {
    ip as *mut c_void
  } else {
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    unsafe {
      _Unwind_FindEnclosingFunction(ip as *mut c_void)
    }
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
      ip as *mut c_void
    }
  }
}

pub(crate) fn frame_symbol_address<T: Frame>(frame: &T) -> *mut c_void {
  Frame::symbol_address(frame)
}

pub(crate) fn frame_ip<T: Frame>(frame: &T) -> usize {
  Frame::ip(frame)
}

#[derive(Clone)]
struct BacktraceSymbolMetadata {
  name:     Option<Vec<u8>>,
  addr:     Option<*mut c_void>,
  lineno:   Option<u32>,
  filename: Option<PathBuf>,
}

trait BacktraceSymbolAccess {
  fn metadata(&self) -> BacktraceSymbolMetadata;
}

fn symbol_metadata_from_backtrace_symbol_access<T>(symbol: &T) -> BacktraceSymbolMetadata
where
  T: BacktraceSymbolAccess,
{
  symbol.metadata()
}

pub trait Symbol: Sized {
  fn name(&self) -> Option<Vec<u8>>;
  fn addr(&self) -> Option<*mut c_void>;
  fn lineno(&self) -> Option<u32>;
  fn filename(&self) -> Option<PathBuf>;
}

impl BacktraceSymbolAccess for backtrace::Symbol {
  fn metadata(&self) -> BacktraceSymbolMetadata {
    BacktraceSymbolMetadata {
      name:     self.name().map(|name| name.as_bytes().to_vec()),
      addr:     self.addr(),
      lineno:   self.lineno(),
      filename: self.filename().map(|filename| filename.to_owned()),
    }
  }
}

impl<T> Symbol for T
where
  T: BacktraceSymbolAccess,
{
  fn name(&self) -> Option<Vec<u8>> {
    symbol_metadata_from_backtrace_symbol_access(self).name
  }

  fn addr(&self) -> Option<*mut c_void> {
    symbol_metadata_from_backtrace_symbol_access(self).addr
  }

  fn lineno(&self) -> Option<u32> {
    symbol_metadata_from_backtrace_symbol_access(self).lineno
  }

  fn filename(&self) -> Option<PathBuf> {
    symbol_metadata_from_backtrace_symbol_access(self).filename
  }
}

pub trait Frame: Sized + Clone {
  type S: Symbol;

  fn resolve_symbol<F: FnMut(&Self::S)>(&self, cb: F);

  fn symbol_address(&self) -> *mut c_void;

  fn ip(&self) -> usize;
}

#[cfg(any(pprof_framehop_backend, pprof_frame_pointer_backend))]
#[derive(Clone, Debug)]
pub struct BacktraceFrame {
  pub(crate) ip: usize,
}

#[cfg(any(pprof_framehop_backend, pprof_frame_pointer_backend))]
impl Frame for BacktraceFrame {
  type S = backtrace::Symbol;

  fn ip(&self) -> usize {
    self.ip
  }

  fn resolve_symbol<F: FnMut(&Self::S)>(&self, cb: F) {
    backtrace::resolve(self.ip as *mut c_void, cb);
  }

  fn symbol_address(&self) -> *mut c_void {
    symbol_address_from_ip(self.ip)
  }
}

pub trait Trace {
  type Frame;

  // init will be called before running the first trace in signal handler
  fn init() {}

  fn trace<F: FnMut(&Self::Frame) -> bool>(_: *mut libc::c_void, cb: F)
  where
    Self: Sized;
}

cfg_if::cfg_if! {
  if #[cfg(pprof_framehop_backend)] {
    pub mod framehop_unwinder;
    pub use framehop_unwinder::Trace as TraceImpl;
  } else if #[cfg(pprof_frame_pointer_backend)] {
    pub mod frame_pointer;
    pub use frame_pointer::Trace as TraceImpl;
  } else {
    mod backtrace_rs;
    pub use backtrace_rs::Trace as TraceImpl;
  }
}

#[cfg(test)]
mod tests {
  use std::path::PathBuf;

  use strict_test_support::TestFailure;
  use strict_test_support::ensure;
  use strict_test_support::ensure_eq;
  use strict_test_support::ensure_some;

  use super::*;

  static TEST_SYMBOL_ANCHOR: u8 = 0;

  fn test_symbol_address() -> *mut c_void {
    std::ptr::addr_of!(TEST_SYMBOL_ANCHOR).cast_mut().cast::<c_void>()
  }

  fn test_frame(ip: usize, symbol_address: *mut c_void) -> TestFrame {
    TestFrame {
      ip,
      symbol_address,
    }
  }

  struct TestBacktraceSymbolAccess(BacktraceSymbolMetadata);

  impl BacktraceSymbolAccess for TestBacktraceSymbolAccess {
    fn metadata(&self) -> BacktraceSymbolMetadata {
      self.0.clone()
    }
  }

  #[derive(Clone)]
  struct TestSymbol {
    label:   &'static [u8],
    address: *mut c_void,
    line:    u32,
    file:    &'static str,
  }

  impl Symbol for TestSymbol {
    fn name(&self) -> Option<Vec<u8>> {
      Some(self.label.to_vec())
    }

    fn addr(&self) -> Option<*mut c_void> {
      Some(self.address)
    }

    fn lineno(&self) -> Option<u32> {
      Some(self.line)
    }

    fn filename(&self) -> Option<PathBuf> {
      Some(PathBuf::from(self.file))
    }
  }

  struct EmptySymbol;

  impl Symbol for EmptySymbol {
    fn name(&self) -> Option<Vec<u8>> {
      None
    }

    fn addr(&self) -> Option<*mut c_void> {
      None
    }

    fn lineno(&self) -> Option<u32> {
      None
    }

    fn filename(&self) -> Option<PathBuf> {
      None
    }
  }

  #[derive(Clone)]
  struct TestFrame {
    ip:             usize,
    symbol_address: *mut c_void,
  }

  impl Frame for TestFrame {
    type S = TestSymbol;

    fn resolve_symbol<F: FnMut(&Self::S)>(&self, mut cb: F) {
      cb(&TestSymbol {
        label:   b"test_symbol",
        address: self.symbol_address,
        line:    7,
        file:    "src/backtrace/mod.rs",
      });
    }

    fn symbol_address(&self) -> *mut c_void {
      self.symbol_address
    }

    fn ip(&self) -> usize {
      self.ip
    }
  }

  struct DefaultInitTrace;

  impl Trace for DefaultInitTrace {
    type Frame = TestFrame;

    fn trace<F: FnMut(&Self::Frame) -> bool>(_: *mut libc::c_void, mut cb: F)
    where
      Self: Sized,
    {
      let frame = TestFrame {
        ip:             0x1234,
        symbol_address: test_symbol_address(),
      };
      cb(&frame);
    }
  }

  struct TwoFrameTrace;

  impl Trace for TwoFrameTrace {
    type Frame = TestFrame;

    fn trace<F: FnMut(&Self::Frame) -> bool>(_: *mut libc::c_void, mut cb: F)
    where
      Self: Sized,
    {
      let first_frame = test_frame(0x1234, test_symbol_address());
      let second_frame = test_frame(0x5678, test_symbol_address());
      let frames = [first_frame, second_frame];
      let _ = frames.iter().try_for_each(|frame| cb(frame).then_some(()).ok_or(()));
    }
  }

  #[test]
  fn frame_helpers_delegate_to_frame_trait_methods() -> std::result::Result<(), TestFailure> {
    let frame = test_frame(0x1234, test_symbol_address());

    ensure_eq(&frame_ip(&frame), &0x1234, "frame ip helper should delegate to Frame::ip")?;
    ensure(
      frame_symbol_address(&frame) == test_symbol_address(),
      "frame symbol address helper should delegate to Frame::symbol_address",
    )
  }

  #[test]
  fn frame_helpers_preserve_null_symbol_addresses() -> std::result::Result<(), TestFailure> {
    let frame = test_frame(0x1234, std::ptr::null_mut());

    ensure(
      frame_symbol_address(&frame).is_null(),
      "frame symbol address helper should preserve null backend symbol addresses",
    )
  }

  #[test]
  fn trace_default_init_is_a_noop_for_backends_without_initialization() -> std::result::Result<(), TestFailure> {
    <DefaultInitTrace as Trace>::init();
    let mut frames_seen = 0usize;

    <DefaultInitTrace as Trace>::trace(std::ptr::null_mut(), |_| {
      frames_seen = frames_seen.saturating_add(1);
      true
    });

    ensure_eq(&frames_seen, &1, "default Trace::init should not block tracing")
  }

  #[test]
  fn trace_callback_controls_iteration() -> std::result::Result<(), TestFailure> {
    let mut stopped_after_first = 0usize;
    <TwoFrameTrace as Trace>::trace(std::ptr::null_mut(), |_| {
      stopped_after_first = stopped_after_first.saturating_add(1);
      false
    });

    let mut continued_until_end = 0usize;
    <TwoFrameTrace as Trace>::trace(std::ptr::null_mut(), |_| {
      continued_until_end = continued_until_end.saturating_add(1);
      true
    });

    ensure_eq(
      &stopped_after_first,
      &1,
      "trace callback returning false should stop iteration after the current frame",
    )?;
    ensure_eq(
      &continued_until_end,
      &2,
      "trace callback returning true should continue through available frames",
    )
  }

  #[test]
  fn backtrace_symbol_access_adapter_preserves_present_metadata() -> std::result::Result<(), TestFailure> {
    let symbol = TestBacktraceSymbolAccess(BacktraceSymbolMetadata {
      name:     Some(b"native_symbol".to_vec()),
      addr:     Some(test_symbol_address()),
      lineno:   Some(19),
      filename: Some(PathBuf::from("src/native.rs")),
    });

    ensure(
      Symbol::name(&symbol) == Some(b"native_symbol".to_vec()),
      "backtrace adapter should preserve native symbol name bytes",
    )?;
    ensure(
      Symbol::addr(&symbol) == Some(test_symbol_address()),
      "backtrace adapter should preserve native symbol address",
    )?;
    ensure(
      Symbol::lineno(&symbol) == Some(19),
      "backtrace adapter should preserve native line number",
    )?;
    ensure(
      Symbol::filename(&symbol) == Some(PathBuf::from("src/native.rs")),
      "backtrace adapter should preserve native filename",
    )
  }

  #[test]
  fn backtrace_symbol_access_adapter_preserves_absent_metadata() -> std::result::Result<(), TestFailure> {
    let symbol = TestBacktraceSymbolAccess(BacktraceSymbolMetadata {
      name:     None,
      addr:     None,
      lineno:   None,
      filename: None,
    });

    ensure(
      Symbol::name(&symbol).is_none(),
      "backtrace adapter should preserve absent symbol names",
    )?;
    ensure(
      Symbol::addr(&symbol).is_none(),
      "backtrace adapter should preserve absent symbol addresses",
    )?;
    ensure(
      Symbol::lineno(&symbol).is_none(),
      "backtrace adapter should preserve absent line numbers",
    )?;
    ensure(
      Symbol::filename(&symbol).is_none(),
      "backtrace adapter should preserve absent filenames",
    )
  }

  #[test]
  fn symbol_trait_exposes_present_metadata() -> std::result::Result<(), TestFailure> {
    let symbol = TestSymbol {
      label:   b"test_symbol",
      address: test_symbol_address(),
      line:    7,
      file:    "src/backtrace/mod.rs",
    };

    ensure(
      symbol.name() == Some(b"test_symbol".to_vec()),
      "symbol trait should expose present names as bytes",
    )?;
    ensure(
      symbol.addr() == Some(test_symbol_address()),
      "symbol trait should expose present addresses",
    )?;
    ensure(symbol.lineno() == Some(7), "symbol trait should expose present line numbers")?;
    ensure(
      symbol.filename() == Some(PathBuf::from("src/backtrace/mod.rs")),
      "symbol trait should expose present filenames",
    )
  }

  #[test]
  fn symbol_trait_allows_absent_metadata() -> std::result::Result<(), TestFailure> {
    let symbol = EmptySymbol;

    ensure(symbol.name().is_none(), "symbol trait should allow absent names")?;
    ensure(symbol.addr().is_none(), "symbol trait should allow absent addresses")?;
    ensure(symbol.lineno().is_none(), "symbol trait should allow absent line numbers")?;
    ensure(symbol.filename().is_none(), "symbol trait should allow absent filenames")
  }

  #[test]
  fn test_frame_resolves_configured_symbol_metadata() -> std::result::Result<(), TestFailure> {
    let frame = test_frame(0x1234, test_symbol_address());
    let mut symbols = Vec::new();

    frame.resolve_symbol(|symbol| symbols.push(symbol.clone()));

    let symbol = ensure_some(symbols.first(), "test frame should resolve configured symbol")?;
    ensure(
      symbol.name() == Some(b"test_symbol".to_vec()),
      "resolved test symbol should expose its name",
    )?;
    ensure(
      symbol.addr() == Some(test_symbol_address()),
      "resolved test symbol should expose its address",
    )?;
    ensure(symbol.lineno() == Some(7), "resolved test symbol should expose its line number")?;
    ensure(
      symbol.filename() == Some(PathBuf::from("src/backtrace/mod.rs")),
      "resolved test symbol should expose its filename",
    )
  }

  #[cfg(any(pprof_framehop_backend, pprof_frame_pointer_backend))]
  #[test]
  fn backtrace_frame_exposes_instruction_pointer() -> std::result::Result<(), TestFailure> {
    let frame = BacktraceFrame {
      ip: 0x1234
    };

    ensure_eq(&frame_ip(&frame), &0x1234, "shared backend frame should expose its ip")
  }

  #[cfg(all(
    any(pprof_framehop_backend, pprof_frame_pointer_backend),
    not(any(target_os = "macos", target_os = "ios"))
  ))]
  #[test]
  fn non_apple_symbol_address_resolves_null_instruction_pointer_to_null() -> std::result::Result<(), TestFailure> {
    ensure(
      symbol_address_from_ip(0).is_null(),
      "non-apple backend symbol address should preserve null instruction pointers as null",
    )
  }

  #[cfg(all(
    any(pprof_framehop_backend, pprof_frame_pointer_backend),
    any(target_os = "macos", target_os = "ios")
  ))]
  #[test]
  fn apple_backtrace_frame_uses_instruction_pointer_as_symbol_address() -> std::result::Result<(), TestFailure> {
    let frame = BacktraceFrame {
      ip: 0x1234
    };

    ensure(
      frame_symbol_address(&frame).addr() == 0x1234,
      "apple backend frame should use instruction pointer as symbol address",
    )
  }
}
