//! Shared workspace-location helpers for local extensions.

use std::env;
use std::path::Path;
use std::path::PathBuf;

/// Resolve the repository root from the compile-time `stask` manifest directory.
///
/// # Errors
///
/// Returns a workflow error if Cargo reports an `stask` manifest directory
/// without a parent repository root.
pub(crate) fn root() -> template_core::Result<PathBuf> {
  let stask_manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
  let Some(root) = stask_manifest_dir.parent() else {
    return Err(crate::error::workflow(format!(
      "`{}` has no parent workspace root",
      stask_manifest_dir.display()
    )));
  };
  Ok(root.to_path_buf())
}
