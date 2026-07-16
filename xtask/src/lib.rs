//! Consumer-owned repository extension composition.
//!
//! Standard repository workflows execute through the installed `template`
//! binary. This crate compiles only the guarded local `x` registry.

use std::process::ExitCode;

mod error;
pub mod extensions;
pub mod matrix;
pub mod proto;
#[cfg(test)]
mod test_support;
pub mod workspace;

/// Run the guarded repository-specific extension surface.
#[must_use]
pub fn run() -> ExitCode {
  template_xtask::run_with_extensions(extensions::commands())
}

#[cfg(test)]
mod tests {
  use strict_test_support::TestFailure;
  use strict_test_support::ensure;
  use strict_test_support::ensure_ok;
  use strict_test_support::ensure_some;
  use template_core::cli::command::CommandSurface;

  use super::extensions;

  #[test]
  fn extension_registry_exposes_the_local_x_router() -> Result<(), TestFailure> {
    let command_set = ensure_ok(
      extensions::commands(),
      "the local extension registry must build",
    )?;
    ensure(
      command_set.surface() == CommandSurface::XtaskExtension,
      "the local command set must remain on the extension surface",
    )?;

    let command = ensure_some(command_set.find("x"), "local extension command set must expose the x router")?;
    ensure(
      command.description() == "Run a consumer-registered extension command",
      "local extension router must keep its user-facing help description",
    )
  }
}
