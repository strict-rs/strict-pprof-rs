#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BackendCfg {
  Framehop,
  FramePointer,
}

impl BackendCfg {
  pub(crate) fn cargo_cfg_directive(self) -> &'static str {
    match self {
      Self::Framehop => "cargo:rustc-cfg=pprof_framehop_backend",
      Self::FramePointer => "cargo:rustc-cfg=pprof_frame_pointer_backend",
    }
  }
}

pub(crate) fn selected_backend_cfg(
  target_arch: &str,
  target_os: &str,
  framehop_enabled: bool,
  frame_pointer_enabled: bool,
) -> Option<BackendCfg> {
  let framehop_supported = ["x86_64", "aarch64"].contains(&target_arch) && ["linux", "macos"].contains(&target_os);
  let frame_pointer_supported = ["x86_64", "aarch64", "riscv64", "loongarch64"].contains(&target_arch);

  if framehop_enabled && framehop_supported {
    Some(BackendCfg::Framehop)
  } else if frame_pointer_enabled && frame_pointer_supported {
    Some(BackendCfg::FramePointer)
  } else {
    None
  }
}
