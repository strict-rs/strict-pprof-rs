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
  use std::sync::Mutex;

  use super::*;

  static VALIDATE_LOCK: Mutex<()> = Mutex::new(());

  #[test]
  fn validate_stack() {
    let _guard = VALIDATE_LOCK.lock().unwrap();
    let i = 0;

    assert!(validate(&i as *const _ as *const libc::c_void));
  }

  #[test]
  fn validate_heap() {
    let _guard = VALIDATE_LOCK.lock().unwrap();
    let vec = vec![0; 1000];

    for i in vec.iter() {
      assert!(validate(i as *const _ as *const libc::c_void));
    }
  }

  #[test]
  fn failed_validate() {
    let _guard = VALIDATE_LOCK.lock().unwrap();
    assert!(!validate(std::ptr::null::<libc::c_void>()));
    assert!(!validate(-1_i32 as usize as *const libc::c_void))
  }

  #[test]
  fn validate_recovers_after_pipe_descriptors_are_closed() {
    let _guard = VALIDATE_LOCK.lock().unwrap();
    let i = 0;

    open_pipe().unwrap();
    let read_fd = MEM_VALIDATE_PIPE.read_fd.load(Ordering::SeqCst);
    let write_fd = MEM_VALIDATE_PIPE.write_fd.load(Ordering::SeqCst);
    close_raw_fd(read_fd);
    close_raw_fd(write_fd);

    assert!(validate(&i as *const _ as *const libc::c_void));
    assert!(MEM_VALIDATE_PIPE.read_fd.load(Ordering::SeqCst) >= 0);
    assert!(MEM_VALIDATE_PIPE.write_fd.load(Ordering::SeqCst) >= 0);
  }
}
