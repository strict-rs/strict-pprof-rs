// Copyright 2022 TiKV Project Authors. Licensed under Apache-2.0.

use std::ptr::null_mut;

use crate::addr_validate::validate;

type Frame = super::BacktraceFrame;

/// helper to read a pointer from a potentially unaligned address
///
/// # Safety
///
/// Same as [`std::ptr::read_unaligned`]
#[inline]
unsafe fn read_ptr<T>(ptr: *const T) -> T {
  if ptr.align_offset(std::mem::align_of::<T>()) == 0 {
    unsafe { std::ptr::read(ptr) }
  } else {
    unsafe { std::ptr::read_unaligned(ptr) }
  }
}

#[cfg(target_os = "macos")]
unsafe fn macos_mcontext(ucontext: *mut libc::ucontext_t) -> Option<libc::mcontext_t> {
  let mcontext = unsafe { (*ucontext).uc_mcontext };
  if mcontext.is_null() { None } else { Some(mcontext) }
}

pub struct Trace {}
impl super::Trace for Trace {
  type Frame = Frame;

  fn trace<F: FnMut(&Self::Frame) -> bool>(ucontext: *mut libc::c_void, mut cb: F) {
    let ucontext: *mut libc::ucontext_t = ucontext as *mut libc::ucontext_t;
    if ucontext.is_null() {
      return;
    }

    #[cfg(all(target_arch = "x86_64", target_os = "linux"))]
    let frame_pointer = unsafe { (*ucontext).uc_mcontext.gregs[libc::REG_RBP as usize] as usize };

    #[cfg(all(target_arch = "x86_64", target_os = "macos"))]
    let frame_pointer = unsafe { macos_mcontext(ucontext).map_or(0, |mcontext| (*mcontext).__ss.__rbp as usize) };

    #[cfg(all(target_arch = "aarch64", target_os = "linux"))]
    let frame_pointer = unsafe { (*ucontext).uc_mcontext.regs[29] as usize };

    #[cfg(all(target_arch = "aarch64", target_os = "macos"))]
    let frame_pointer = unsafe { macos_mcontext(ucontext).map_or(0, |mcontext| (*mcontext).__ss.__fp as usize) };

    #[cfg(all(target_arch = "riscv64", target_os = "linux"))]
    let frame_pointer = unsafe { (*ucontext).uc_mcontext.__gregs[libc::REG_S0] as usize };

    #[cfg(all(target_arch = "loongarch64", target_os = "linux"))]
    let frame_pointer = unsafe { (*ucontext).uc_mcontext.__gregs[22] as usize };

    let mut frame_pointer = frame_pointer as *mut FramePointerLayout;

    let mut last_frame_pointer: *mut FramePointerLayout = null_mut();
    loop {
      // The stack grow from high address to low address.
      // but we don't have a reasonable assumption for the hightest address
      // the `__libc_stack_end` is not thread-local, and only represent the
      // stack end of the main thread. For other thread, their stacks are allocated
      // by the `pthread`.
      //
      // TODO: If we can hook the thread creation, we will have chance to get the
      // stack end through `pthread_get_attr`.

      // the frame pointer should never be smaller than the former one.
      if !last_frame_pointer.is_null() && frame_pointer < last_frame_pointer {
        break;
      }

      if !validate(frame_pointer as *const libc::c_void) {
        break;
      }
      last_frame_pointer = frame_pointer;

      // iterate to the next frame
      let frame = Frame {
        ip: unsafe { read_ptr(frame_pointer).ret },
      };

      if !cb(&frame) {
        break;
      }
      frame_pointer = unsafe { read_ptr(frame_pointer).frame_pointer };
    }
  }
}

#[repr(C)]
struct FramePointerLayout {
  frame_pointer: *mut FramePointerLayout,
  ret:           usize,
}

#[cfg(test)]
mod tests {
  use strict_test_support::TestFailure;
  use strict_test_support::ensure;
  use strict_test_support::ensure_eq;
  use strict_test_support::ensure_ok;
  use strict_test_support::ensure_some;

  use super::*;

  #[repr(C, align(64))]
  struct AlignToSixtyFour([u8; 64]);

  impl Default for AlignToSixtyFour {
    fn default() -> Self {
      AlignToSixtyFour([0; 64])
    }
  }

  #[test]
  fn test_read_ptr_aligned() -> std::result::Result<(), TestFailure> {
    let x = AlignToSixtyFour::default();
    let ptr = &x.0[0] as *const u8;
    let actual = unsafe { read_ptr(ptr) };
    ensure_eq(&actual, &x.0[0], "aligned pointer read should return byte")
  }

  #[test]
  fn test_read_ptr_unaligned() -> std::result::Result<(), TestFailure> {
    let mut x = AlignToSixtyFour::default();
    let expected = usize::MAX / 2;
    for (target_byte, source_byte) in x.0[1..9].iter_mut().zip(expected.to_ne_bytes()) {
      *target_byte = source_byte;
    }
    let ptr: *const usize = unsafe { std::mem::transmute(&x.0[1] as *const u8) };
    let actual = unsafe { read_ptr(ptr) };
    ensure_eq(&actual, &expected, "unaligned pointer read should return usize")
  }

  #[test]
  fn trace_returns_without_null_context() -> std::result::Result<(), TestFailure> {
    let mut frame_seen = false;

    <Trace as super::super::Trace>::trace(std::ptr::null_mut(), |_| {
      frame_seen = true;
      true
    });

    ensure(!frame_seen, "null frame-pointer context should not invoke callbacks")
  }

  #[cfg(all(target_arch = "x86_64", target_os = "linux"))]
  enum TestFramePointerChain {
    Increasing,
    Decreasing,
  }

  #[cfg(all(target_arch = "x86_64", target_os = "linux"))]
  fn trace_from_start_frame_pointer<F>(
    start_frame: *mut FramePointerLayout,
    mut callback: F,
  ) -> std::result::Result<Vec<usize>, TestFailure>
  where
    F: FnMut(&Frame) -> bool,
  {
    let mut context: libc::ucontext_t = unsafe { std::mem::zeroed() };
    let register_index = ensure_ok(
      usize::try_from(libc::REG_RBP),
      "linux frame-pointer register index should fit usize",
    )?;
    context.uc_mcontext.gregs[register_index] = ensure_ok(start_frame.addr().try_into(), "test frame-pointer address should fit greg_t")?;
    let mut ips = Vec::new();

    <Trace as super::super::Trace>::trace(std::ptr::addr_of_mut!(context).cast::<libc::c_void>(), |frame| {
      ips.push(frame.ip);
      callback(frame)
    });

    Ok(ips)
  }

  #[cfg(all(target_arch = "x86_64", target_os = "linux"))]
  fn trace_test_frame_pointer_chain<F>(chain: TestFramePointerChain, callback: F) -> std::result::Result<Vec<usize>, TestFailure>
  where
    F: FnMut(&Frame) -> bool,
  {
    let mut frames = Box::new([
      std::mem::MaybeUninit::<FramePointerLayout>::uninit(),
      std::mem::MaybeUninit::<FramePointerLayout>::uninit(),
    ]);
    let first_frame = frames[0].as_mut_ptr();
    let second_frame = frames[1].as_mut_ptr();
    let start_frame = match chain {
      TestFramePointerChain::Increasing => {
        unsafe {
          first_frame.write(FramePointerLayout {
            frame_pointer: second_frame,
            ret:           0x1111,
          });
          second_frame.write(FramePointerLayout {
            frame_pointer: std::ptr::null_mut(),
            ret:           0x2222,
          });
        }
        first_frame
      }
      TestFramePointerChain::Decreasing => {
        unsafe {
          first_frame.write(FramePointerLayout {
            frame_pointer: std::ptr::null_mut(),
            ret:           0x1111,
          });
          second_frame.write(FramePointerLayout {
            frame_pointer: first_frame,
            ret:           0x2222,
          });
        }
        second_frame
      }
    };

    trace_from_start_frame_pointer(start_frame, callback)
  }

  #[cfg(all(target_arch = "x86_64", target_os = "linux"))]
  #[test]
  fn trace_walks_valid_frame_pointer_chain() -> std::result::Result<(), TestFailure> {
    let ips = trace_test_frame_pointer_chain(TestFramePointerChain::Increasing, |_| true)?;

    ensure(
      ips == vec![0x1111, 0x2222],
      "frame-pointer trace should walk linked frame pointers in order",
    )
  }

  #[cfg(all(target_arch = "x86_64", target_os = "linux"))]
  #[test]
  fn trace_stops_when_callback_returns_false() -> std::result::Result<(), TestFailure> {
    let ips = trace_test_frame_pointer_chain(TestFramePointerChain::Increasing, |_| false)?;

    ensure(ips == vec![0x1111], "frame-pointer trace should stop when callback returns false")
  }

  #[cfg(all(target_arch = "x86_64", target_os = "linux"))]
  #[test]
  fn trace_stops_before_decreasing_frame_pointer() -> std::result::Result<(), TestFailure> {
    let ips = trace_test_frame_pointer_chain(TestFramePointerChain::Decreasing, |_| true)?;

    ensure(
      ips == vec![0x2222],
      "frame-pointer trace should stop before walking to a lower frame pointer",
    )
  }

  #[cfg(all(target_arch = "x86_64", target_os = "linux"))]
  #[test]
  fn trace_stops_before_invalid_start_frame_pointer() -> std::result::Result<(), TestFailure> {
    let ips = trace_from_start_frame_pointer(std::ptr::null_mut(), |_| true)?;

    ensure(
      ips.is_empty(),
      "valid frame-pointer context with an invalid starting frame pointer should not invoke callbacks",
    )
  }

  #[cfg(all(target_arch = "x86_64", target_os = "linux"))]
  #[test]
  fn trace_reads_unaligned_frame_pointer_layout() -> std::result::Result<(), TestFailure> {
    let byte_count = ensure_some(
      std::mem::size_of::<FramePointerLayout>().checked_add(1),
      "unaligned frame-pointer fixture size should fit usize",
    )?;
    let mut bytes = vec![0_u8; byte_count];
    let frame_pointer = unsafe { bytes.as_mut_ptr().add(1).cast::<FramePointerLayout>() };
    unsafe {
      std::ptr::write_unaligned(frame_pointer, FramePointerLayout {
        frame_pointer: std::ptr::null_mut(),
        ret:           0x3333,
      });
    }

    let ips = trace_from_start_frame_pointer(frame_pointer, |_| true)?;

    ensure(
      ips == vec![0x3333],
      "frame-pointer trace should read a misaligned frame-pointer record with the unaligned reader",
    )
  }
}
