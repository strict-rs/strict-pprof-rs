impl super::Frame for backtrace::Frame {
  type S = backtrace::Symbol;

  fn ip(&self) -> usize {
    self.ip() as usize
  }

  fn resolve_symbol<F: FnMut(&Self::S)>(&self, cb: F) {
    backtrace::resolve_frame(self, cb);
  }

  fn symbol_address(&self) -> *mut libc::c_void {
    self.symbol_address()
  }
}

pub struct Trace {}

impl super::Trace for Trace {
  type Frame = backtrace::Frame;

  fn trace<F: FnMut(&Self::Frame) -> bool>(_: *mut libc::c_void, cb: F) {
    unsafe { backtrace::trace_unsynchronized(cb) }
  }
}

#[cfg(test)]
mod tests {
  use strict_test_support::TestFailure;
  use strict_test_support::ensure;
  use strict_test_support::ensure_eq;
  use strict_test_support::ensure_some;

  use super::Trace;
  use crate::backtrace::Frame as FrameTrait;
  use crate::backtrace::Trace as TraceTrait;

  fn capture_trace<F>(mut callback: F) -> Vec<usize>
  where
    F: FnMut(&backtrace::Frame) -> bool,
  {
    let mut ips = Vec::new();

    <Trace as TraceTrait>::trace(std::ptr::null_mut(), |frame| {
      ips.push(FrameTrait::ip(frame));
      callback(frame)
    });

    ips
  }

  #[test]
  fn trace_invokes_callback_with_native_frame() -> std::result::Result<(), TestFailure> {
    let ips = capture_trace(|_| false);
    let first_ip = ensure_some(ips.first(), "fallback backtrace should provide at least one frame")?;

    ensure(
      *first_ip > 0,
      "fallback backtrace frames should expose nonzero instruction pointers",
    )
  }

  #[test]
  fn trace_stops_when_callback_returns_false() -> std::result::Result<(), TestFailure> {
    let ips = capture_trace(|_| false);

    ensure_eq(&ips.len(), &1, "fallback backtrace should stop after callback returns false")
  }

  #[test]
  fn trace_continues_while_callback_returns_true() -> std::result::Result<(), TestFailure> {
    let ips = capture_trace(|_| true);

    ensure(ips.len() > 1, "fallback backtrace should continue while callback returns true")
  }

  #[test]
  fn frame_trait_matches_native_frame_accessors() -> std::result::Result<(), TestFailure> {
    let mut trait_ip = 0usize;
    let mut native_ip = 0usize;
    let mut trait_symbol_address = std::ptr::null_mut();
    let mut native_symbol_address = std::ptr::null_mut();

    <Trace as TraceTrait>::trace(std::ptr::null_mut(), |frame| {
      trait_ip = FrameTrait::ip(frame);
      native_ip = frame.ip().addr();
      trait_symbol_address = FrameTrait::symbol_address(frame);
      native_symbol_address = frame.symbol_address();
      false
    });

    ensure_eq(&trait_ip, &native_ip, "fallback frame trait should expose the native frame ip")?;
    ensure(
      trait_symbol_address == native_symbol_address,
      "fallback frame trait should expose the native symbol address",
    )
  }

  #[test]
  fn frame_symbol_resolution_returns_control_to_caller() -> std::result::Result<(), TestFailure> {
    let mut resolution_completed = false;

    <Trace as TraceTrait>::trace(std::ptr::null_mut(), |frame| {
      FrameTrait::resolve_symbol(frame, |_| {});
      resolution_completed = true;
      false
    });

    ensure(
      resolution_completed,
      "fallback frame symbol resolution should return control to the caller",
    )
  }
}
