#[path = "../build_support.rs"]
mod build_support;

use build_support::BackendCfg;
use build_support::selected_backend_cfg;
use strict_test_support::TestFailure;
use strict_test_support::ensure;
use strict_test_support::ensure_eq;

struct BackendFeatures {
  framehop:      bool,
  frame_pointer: bool,
}

struct BackendSelectionCase {
  target_arch:      &'static str,
  target_os:        &'static str,
  features:         BackendFeatures,
  expected_backend: Option<BackendCfg>,
  context:          &'static str,
}

fn ensure_backend_selection(case: &BackendSelectionCase) -> Result<(), TestFailure> {
  ensure(
    selected_backend_cfg(
      case.target_arch, case.target_os, case.features.framehop, case.features.frame_pointer,
    ) == case.expected_backend,
    case.context,
  )
}

#[test]
fn selects_backend_cfg_from_target_and_enabled_features() -> Result<(), TestFailure> {
  let cases = [
    BackendSelectionCase {
      target_arch:      "x86_64",
      target_os:        "linux",
      features:         BackendFeatures {
        framehop:      true,
        frame_pointer: false,
      },
      expected_backend: Some(BackendCfg::Framehop),
      context:          "framehop feature should select framehop backend on supported linux targets",
    },
    BackendSelectionCase {
      target_arch:      "aarch64",
      target_os:        "macos",
      features:         BackendFeatures {
        framehop:      true,
        frame_pointer: false,
      },
      expected_backend: Some(BackendCfg::Framehop),
      context:          "framehop feature should select framehop backend on supported macos targets",
    },
    BackendSelectionCase {
      target_arch:      "x86_64",
      target_os:        "linux",
      features:         BackendFeatures {
        framehop:      true,
        frame_pointer: true,
      },
      expected_backend: Some(BackendCfg::Framehop),
      context:          "framehop backend should take precedence when both backend features are enabled and supported",
    },
    BackendSelectionCase {
      target_arch:      "x86_64",
      target_os:        "linux",
      features:         BackendFeatures {
        framehop:      false,
        frame_pointer: true,
      },
      expected_backend: Some(BackendCfg::FramePointer),
      context:          "frame-pointer feature should select frame-pointer backend when framehop is disabled",
    },
    BackendSelectionCase {
      target_arch:      "riscv64",
      target_os:        "linux",
      features:         BackendFeatures {
        framehop:      true,
        frame_pointer: true,
      },
      expected_backend: Some(BackendCfg::FramePointer),
      context:          "frame-pointer backend should be used when framehop is enabled but unsupported for the target arch",
    },
    BackendSelectionCase {
      target_arch:      "loongarch64",
      target_os:        "freebsd",
      features:         BackendFeatures {
        framehop:      false,
        frame_pointer: true,
      },
      expected_backend: Some(BackendCfg::FramePointer),
      context:          "frame-pointer backend support should depend on target arch rather than target os",
    },
    BackendSelectionCase {
      target_arch:      "x86_64",
      target_os:        "linux",
      features:         BackendFeatures {
        framehop:      false,
        frame_pointer: false,
      },
      expected_backend: None,
      context:          "disabled backend features should not emit a backend cfg",
    },
    BackendSelectionCase {
      target_arch:      "wasm32",
      target_os:        "unknown",
      features:         BackendFeatures {
        framehop:      true,
        frame_pointer: true,
      },
      expected_backend: None,
      context:          "unsupported target arch should not emit a backend cfg",
    },
    BackendSelectionCase {
      target_arch:      "x86_64",
      target_os:        "freebsd",
      features:         BackendFeatures {
        framehop:      true,
        frame_pointer: false,
      },
      expected_backend: None,
      context:          "framehop-only builds should not fall back to frame-pointer when frame-pointer feature is disabled",
    },
  ];

  for case in &cases {
    ensure_backend_selection(case)?;
  }

  Ok(())
}

#[test]
fn backend_cfgs_expose_expected_cargo_directives() -> Result<(), TestFailure> {
  ensure_eq(
    &BackendCfg::Framehop.cargo_cfg_directive(),
    &"cargo:rustc-cfg=pprof_framehop_backend",
    "framehop backend should emit the framehop cfg directive",
  )?;
  ensure_eq(
    &BackendCfg::FramePointer.cargo_cfg_directive(),
    &"cargo:rustc-cfg=pprof_frame_pointer_backend",
    "frame-pointer backend should emit the frame-pointer cfg directive",
  )
}
