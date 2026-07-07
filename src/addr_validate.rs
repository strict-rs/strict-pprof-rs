use std::mem::size_of;
use std::os::fd::BorrowedFd;
use std::os::fd::IntoRawFd;
use std::os::fd::RawFd;
use std::sync::atomic::AtomicI32;
use std::sync::atomic::Ordering;

use nix::errno::Errno;
use nix::unistd::close;
use nix::unistd::read;
use nix::unistd::write;

struct Pipes {
  read_fd:  AtomicI32,
  write_fd: AtomicI32,
}

static MEM_VALIDATE_PIPE: Pipes = Pipes {
  read_fd:  AtomicI32::new(-1),
  write_fd: AtomicI32::new(-1),
};

#[inline]
#[cfg(any(target_os = "android", target_os = "linux"))]
fn create_pipe() -> nix::Result<(RawFd, RawFd)> {
  use nix::fcntl::OFlag;
  use nix::unistd::pipe2;

  let (read_fd, write_fd) = pipe2(OFlag::O_CLOEXEC | OFlag::O_NONBLOCK)?;
  Ok((read_fd.into_raw_fd(), write_fd.into_raw_fd()))
}

#[inline]
#[cfg(any(target_os = "macos", target_os = "freebsd"))]
fn create_pipe() -> nix::Result<(RawFd, RawFd)> {
  use std::os::fd::AsFd;

  use nix::fcntl::FcntlArg;
  use nix::fcntl::FdFlag;
  use nix::fcntl::OFlag;
  use nix::fcntl::fcntl;
  use nix::unistd::pipe;

  fn set_flags(fd: BorrowedFd<'_>) -> nix::Result<()> {
    let mut flags = FdFlag::from_bits(fcntl(fd, FcntlArg::F_GETFD)?).unwrap();
    flags |= FdFlag::FD_CLOEXEC;
    fcntl(fd, FcntlArg::F_SETFD(flags))?;
    let mut flags = OFlag::from_bits(fcntl(fd, FcntlArg::F_GETFL)?).unwrap();
    flags |= OFlag::O_NONBLOCK;
    fcntl(fd, FcntlArg::F_SETFL(flags))?;
    Ok(())
  }

  let (read_fd, write_fd) = pipe()?;
  set_flags(read_fd.as_fd())?;
  set_flags(write_fd.as_fd())?;
  Ok((read_fd.into_raw_fd(), write_fd.into_raw_fd()))
}

fn close_raw_fd(fd: RawFd) {
  if fd >= 0 {
    let _ = close(fd);
  }
}

fn read_raw_fd(fd: RawFd, buf: &mut [u8]) -> nix::Result<usize> {
  if fd < 0 {
    return Err(Errno::EBADF);
  }

  read(unsafe { BorrowedFd::borrow_raw(fd) }, buf)
}

fn write_raw_fd(fd: RawFd, buf: &[u8]) -> nix::Result<usize> {
  if fd < 0 {
    return Err(Errno::EBADF);
  }

  write(unsafe { BorrowedFd::borrow_raw(fd) }, buf)
}

fn open_pipe() -> nix::Result<()> {
  let old_read_fd = MEM_VALIDATE_PIPE.read_fd.swap(-1, Ordering::SeqCst);
  let old_write_fd = MEM_VALIDATE_PIPE.write_fd.swap(-1, Ordering::SeqCst);
  close_raw_fd(old_read_fd);
  close_raw_fd(old_write_fd);

  let (read_fd, write_fd) = create_pipe()?;

  MEM_VALIDATE_PIPE.read_fd.store(read_fd, Ordering::SeqCst);
  MEM_VALIDATE_PIPE.write_fd.store(write_fd, Ordering::SeqCst);

  Ok(())
}

// validate whether the address `addr` is readable through `write()` to a pipe
//
// if the second argument of `write(ptr, buf)` is not a valid address, the
// `write()` will return an error the error number should be `EFAULT` in most
// cases, but we regard all errors (except EINTR) as a failure of validation
pub fn validate(addr: *const libc::c_void) -> bool {
  // it's a short circuit for null pointer, as it'll give an error in
  // `std::slice::from_raw_parts` if the pointer is null.
  if addr.is_null() {
    return false;
  }

  const CHECK_LENGTH: usize = 2 * size_of::<*const libc::c_void>() / size_of::<u8>();

  // read data in the pipe
  let read_fd = MEM_VALIDATE_PIPE.read_fd.load(Ordering::SeqCst);
  let valid_read = loop {
    let mut buf = [0u8; CHECK_LENGTH];

    match read_raw_fd(read_fd, &mut buf) {
      Ok(bytes) => break bytes > 0,
      Err(_err @ Errno::EINTR) => continue,
      Err(_err @ Errno::EAGAIN) => break true,
      Err(_) => break false,
    }
  };

  if !valid_read && open_pipe().is_err() {
    return false;
  }

  let write_fd = MEM_VALIDATE_PIPE.write_fd.load(Ordering::SeqCst);
  loop {
    let buf = unsafe { std::slice::from_raw_parts(addr as *const u8, CHECK_LENGTH) };

    match write_raw_fd(write_fd, buf) {
      Ok(bytes) => break bytes > 0,
      Err(_err @ Errno::EINTR) => continue,
      Err(_) => break false,
    }
  }
}

#[cfg(test)]
mod test {
  use parking_lot::Mutex;
  use strict_test_support::TestFailure;
  use strict_test_support::ensure;
  use strict_test_support::ensure_eq;
  use strict_test_support::ensure_ok;

  use super::*;

  static VALIDATE_LOCK: Mutex<()> = Mutex::new(());

  #[test]
  fn raw_fd_helpers_reject_negative_descriptors() -> std::result::Result<(), TestFailure> {
    let mut read_buf = [0_u8; 1];
    let write_buf = [1_u8; 1];

    ensure(
      read_raw_fd(-1, &mut read_buf).err() == Some(Errno::EBADF),
      "negative read file descriptors should be rejected before borrowing",
    )?;
    ensure(
      write_raw_fd(-1, &write_buf).err() == Some(Errno::EBADF),
      "negative write file descriptors should be rejected before borrowing",
    )
  }

  #[test]
  fn open_pipe_installs_connected_read_write_descriptors() -> std::result::Result<(), TestFailure> {
    let _guard = VALIDATE_LOCK.lock();
    ensure_ok(open_pipe(), "validation pipe should open")?;
    let read_fd = MEM_VALIDATE_PIPE.read_fd.load(Ordering::SeqCst);
    let write_fd = MEM_VALIDATE_PIPE.write_fd.load(Ordering::SeqCst);
    let mut received = [0_u8; 1];

    ensure(read_fd >= 0, "opened validation pipe should store a read descriptor")?;
    ensure(write_fd >= 0, "opened validation pipe should store a write descriptor")?;
    ensure_eq(
      &ensure_ok(write_raw_fd(write_fd, &[42]), "validation pipe should accept a byte")?,
      &1,
      "validation pipe write should report one byte",
    )?;
    ensure_eq(
      &ensure_ok(read_raw_fd(read_fd, &mut received), "validation pipe should return a byte")?,
      &1,
      "validation pipe read should report one byte",
    )?;
    ensure(
      received == [42],
      "validation pipe read descriptor should receive bytes written to its paired write descriptor",
    )
  }

  #[test]
  fn validate_writes_probe_bytes_for_readable_address() -> std::result::Result<(), TestFailure> {
    let _guard = VALIDATE_LOCK.lock();
    let readable = 0usize;
    let mut probe_bytes = [0_u8; 64];

    ensure_ok(open_pipe(), "validation pipe should open")?;
    ensure(
      validate(&readable as *const _ as *const libc::c_void),
      "readable stack address should validate",
    )?;

    let read_fd = MEM_VALIDATE_PIPE.read_fd.load(Ordering::SeqCst);
    let bytes = ensure_ok(
      read_raw_fd(read_fd, &mut probe_bytes),
      "successful validation should leave probe bytes readable from the pipe",
    )?;
    ensure(bytes > 0, "successful validation should write at least one probe byte to the pipe")
  }

  #[test]
  fn validate_stack() -> std::result::Result<(), TestFailure> {
    let _guard = VALIDATE_LOCK.lock();
    let i = 0;

    ensure(validate(&i as *const _ as *const libc::c_void), "stack address should validate")
  }

  #[test]
  fn validate_heap() -> std::result::Result<(), TestFailure> {
    let _guard = VALIDATE_LOCK.lock();
    let vec = vec![0; 1000];

    for i in vec.iter() {
      ensure(validate(i as *const _ as *const libc::c_void), "heap address should validate")?;
    }
    Ok(())
  }

  #[test]
  fn failed_validate() -> std::result::Result<(), TestFailure> {
    let _guard = VALIDATE_LOCK.lock();
    ensure(!validate(std::ptr::null::<libc::c_void>()), "null pointer should not validate")?;
    ensure(
      !validate(-1_i32 as usize as *const libc::c_void),
      "invalid address should not validate",
    )
  }

  #[test]
  fn validate_recovers_after_pipe_descriptors_are_closed() -> std::result::Result<(), TestFailure> {
    let _guard = VALIDATE_LOCK.lock();
    let i = 0;

    ensure_ok(open_pipe(), "pipe should open before forced close")?;
    let read_fd = MEM_VALIDATE_PIPE.read_fd.load(Ordering::SeqCst);
    let write_fd = MEM_VALIDATE_PIPE.write_fd.load(Ordering::SeqCst);
    close_raw_fd(read_fd);
    close_raw_fd(write_fd);

    ensure(validate(&i as *const _ as *const libc::c_void), "validation should reopen the pipe")?;
    ensure(
      MEM_VALIDATE_PIPE.read_fd.load(Ordering::SeqCst) >= 0,
      "read file descriptor should be reopened",
    )?;
    ensure(
      MEM_VALIDATE_PIPE.write_fd.load(Ordering::SeqCst) >= 0,
      "write file descriptor should be reopened",
    )
  }
}
