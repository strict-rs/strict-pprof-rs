// Copyright 2019 TiKV Project Authors. Licensed under Apache-2.0.

use std::os::raw::c_int;
use std::ptr::null_mut;
use std::time::Duration;
use std::time::Instant;
use std::time::SystemTime;

use crate::frames::current_system_time;

#[repr(C)]
#[derive(Clone)]
struct Timeval {
  pub tv_sec:  i64,
  pub tv_usec: i64,
}

#[repr(C)]
#[derive(Clone)]
struct Itimerval {
  pub it_interval: Timeval,
  pub it_value:    Timeval,
}

unsafe extern "C" {
  fn setitimer(which: c_int, new_value: *mut Itimerval, old_value: *mut Itimerval) -> c_int;
}

const ITIMER_PROF: c_int = 2;

pub struct Timer {
  pub frequency:     c_int,
  pub start_time:    SystemTime,
  pub start_instant: Instant,
}

impl Timer {
  pub fn new(frequency: c_int) -> Timer {
    let interval = 1e6 as i64 / i64::from(frequency);
    let it_interval = Timeval {
      tv_sec:  interval / 1e6 as i64,
      tv_usec: interval % 1e6 as i64,
    };
    let it_value = it_interval.clone();

    unsafe {
      setitimer(
        ITIMER_PROF,
        &mut Itimerval {
          it_interval,
          it_value,
        },
        null_mut(),
      )
    };

    Timer {
      frequency,
      start_time: current_system_time(),
      start_instant: Instant::now(),
    }
  }

  /// Returns a `ReportTiming` struct having this timer's frequency and start
  /// time; and the time elapsed since its creation as duration.
  pub fn timing(&self) -> ReportTiming {
    ReportTiming {
      frequency:  self.frequency,
      start_time: self.start_time,
      duration:   self.start_instant.elapsed(),
    }
  }
}

impl Drop for Timer {
  fn drop(&mut self) {
    let it_interval = Timeval {
      tv_sec: 0, tv_usec: 0
    };
    let it_value = it_interval.clone();
    unsafe {
      setitimer(
        ITIMER_PROF,
        &mut Itimerval {
          it_interval,
          it_value,
        },
        null_mut(),
      )
    };
  }
}

/// Timing metadata for a collected report.
#[derive(Clone)]
pub struct ReportTiming {
  /// Frequency at which samples were collected.
  pub frequency:  i32,
  /// Collection start time.
  pub start_time: SystemTime,
  /// Collection duration.
  pub duration:   Duration,
}

impl Default for ReportTiming {
  fn default() -> Self {
    Self {
      frequency:  1,
      start_time: SystemTime::UNIX_EPOCH,
      duration:   Default::default(),
    }
  }
}

#[cfg(test)]
mod tests {
  use strict_test_support::TestFailure;
  use strict_test_support::ensure;
  use strict_test_support::ensure_eq;

  use super::*;

  #[test]
  fn report_timing_default_is_epoch_with_unit_frequency() -> std::result::Result<(), TestFailure> {
    let timing = ReportTiming::default();

    ensure_eq(&timing.frequency, &1, "default frequency should be one")?;
    ensure(timing.start_time == SystemTime::UNIX_EPOCH, "default start time should be epoch")?;
    ensure(timing.duration == Duration::default(), "default duration should be zero")
  }

  #[test]
  fn timer_records_frequency_start_time_and_elapsed_duration() -> std::result::Result<(), TestFailure> {
    let timer = Timer::new(1);
    let timing = timer.timing();

    ensure_eq(&timing.frequency, &1, "timer timing should preserve frequency")?;
    ensure(
      timing.start_time.duration_since(SystemTime::UNIX_EPOCH).is_ok(),
      "timer start time should be after unix epoch",
    )?;
    ensure(
      timing.duration <= timer.start_instant.elapsed(),
      "timing duration should not exceed current elapsed time",
    )
  }
}
