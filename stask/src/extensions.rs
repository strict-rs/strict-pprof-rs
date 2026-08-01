//! Consumer-owned extension registry for `just x <name>` commands.

use template_core::cli::command::CommandSet;
use template_core::cli::context::CommandContext;

use crate::matrix;
use crate::proto;

/// Every extension command this repository exposes under `just x <name>`.
#[derive(Clone, Debug)]
pub(crate) enum ProjectCommand {
  /// Run the protobuf/prost verification matrix.
  Matrix(matrix::Args),
  /// Run protobuf setup, update, and generation helpers.
  Proto(proto::Args),
}

impl ProjectCommand {
  /// Run the command selected by the extension parser.
  ///
  /// # Errors
  ///
  /// Propagates the selected command's error.
  #[allow(
    clippy::single_call_fn,
    reason = "extension dispatch is intentionally named so adding a command keeps parsing and execution boundaries separate"
  )]
  fn run(self, context: &CommandContext) -> template_core::Result<()> {
    match self {
      Self::Matrix(args) => matrix::execute(context, args),
      Self::Proto(args) => proto::execute(context, args),
    }
  }
}

/// Build the local extension command set.
///
/// # Errors
///
/// Returns a typed registration error if either nested command or the
/// controlled `x` router metadata is invalid.
pub fn commands() -> template_stask::Result<CommandSet> {
  template_stask::registry(
    "strict-pprof extensions",
    vec![matrix::command()?, proto::command()?],
    ProjectCommand::run,
  )
}
