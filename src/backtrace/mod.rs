// Copyright 2022 TiKV Project Authors. Licensed under Apache-2.0.

use std::path::PathBuf;

use libc::c_void;

pub trait Symbol: Sized {
  fn name(&self) -> Option<Vec<u8>>;
  fn addr(&self) -> Option<*mut c_void>;
  fn lineno(&self) -> Option<u32>;
  fn filename(&self) -> Option<PathBuf>;
}

impl Symbol for backtrace::Symbol {
  fn name(&self) -> Option<Vec<u8>> {
    self.name().map(|name| name.as_bytes().to_vec())
  }

  fn addr(&self) -> Option<*mut libc::c_void> {
    self.addr()
  }

  fn lineno(&self) -> Option<u32> {
    self.lineno()
  }

  fn filename(&self) -> Option<std::path::PathBuf> {
    self.filename().map(|filename| filename.to_owned())
  }
}

pub trait Frame: Sized + Clone {
  type S: Symbol;

  fn resolve_symbol<F: FnMut(&Self::S)>(&self, cb: F);

  #[allow(dead_code)]
  fn symbol_address(&self) -> *mut c_void;

  #[allow(dead_code)]
  fn ip(&self) -> usize;
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
