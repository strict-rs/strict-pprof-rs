//! Typed failures owned by the repository-specific extension surface.

/// Failure carrying a repository automation invariant or toolchain diagnostic.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ProjectError {
  /// A local workflow invariant failed.
  #[error("{message}")]
  Workflow {
    /// User-facing invariant or validation detail.
    message: String,
  },
}

/// Preserve a repository workflow diagnostic through the shared runner error.
pub(crate) fn workflow(message: impl Into<String>) -> template_core::CoreError {
  template_core::CoreError::workflow(ProjectError::Workflow {
    message: message.into()
  })
}
