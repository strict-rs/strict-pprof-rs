//! Shared deterministic process fixtures for local extension tests.

use std::path::PathBuf;
use std::sync::Arc;

use strict_standard::EnvironmentChange;
use strict_standard::OutputPolicy;
use strict_standard::ProcessRequest;
use strict_test_support::RecordingEffects;
use strict_test_support::TestFailure;
use strict_test_support::process_output;
use template_core::cli::color::ColorContext;
use template_core::cli::context::CommandContext;
use template_core::cli::output::OutputSink;
use template_core::sys::process::ToolColor;

/// One process result consumed by a local extension test.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProcessFixture<'output> {
  /// Return a successful status with empty captured streams.
  Success,
  /// Return a nonzero status with empty captured streams.
  Failure,
  /// Return a successful status with the specified standard-output text.
  Stdout(&'output str),
}

/// Construct a command context backed by the shared ecosystem recorder.
pub(crate) fn recording_context(
  fixtures: &[ProcessFixture<'_>],
) -> Result<(CommandContext, Arc<RecordingEffects>), TestFailure> {
  let recorder = Arc::new(RecordingEffects::default());
  for fixture in fixtures {
    let output = match *fixture {
      ProcessFixture::Success => process_output(0, Vec::new(), Vec::new())?,
      ProcessFixture::Failure => process_output(1, Vec::new(), Vec::new())?,
      ProcessFixture::Stdout(stdout) => process_output(0, stdout.as_bytes().to_vec(), Vec::new())?,
    };
    recorder.queue_process_result(Ok(output));
  }
  let context = CommandContext::with_effects(
    PathBuf::from("/work/here"),
    ColorContext::captured_auto_for_tests(),
    OutputSink::captured().0,
    Arc::clone(&recorder),
  );
  Ok((context, recorder))
}

/// Build the exact template-policy request expected from a local extension.
pub(crate) fn extension_request(
  context: &CommandContext,
  program: &str,
  arguments: &[&str],
  color: ToolColor,
  stdout: OutputPolicy,
  environment: &[EnvironmentChange],
) -> ProcessRequest {
  let owned_arguments = arguments.iter().map(|argument| (*argument).to_owned()).collect::<Vec<_>>();
  context.process_request(
    program,
    &owned_arguments,
    color,
    stdout,
    OutputPolicy::Inherit,
    environment,
  )
}

/// Build the exact setup request sequence shared by both local commands.
pub(crate) fn expected_setup_requests(context: &CommandContext, protoc: &str) -> Vec<ProcessRequest> {
  vec![
    extension_request(
      context,
      "proto",
      &["install", "protoc"],
      ToolColor::EnvOnly,
      OutputPolicy::Inherit,
      &[],
    ),
    extension_request(
      context,
      "proto",
      &["--reporter", "text", "bin", "protoc"],
      ToolColor::CapturedPlain,
      OutputPolicy::Capture,
      &[],
    ),
    extension_request(
      context,
      protoc,
      &["--version"],
      ToolColor::CapturedPlain,
      OutputPolicy::Capture,
      &[],
    ),
  ]
}

/// Build the exact child environment binding for the validated compiler.
pub(crate) fn protoc_environment(protoc: &str) -> [EnvironmentChange; 1] {
  [EnvironmentChange::Set {
    name:  "PROTOC".into(),
    value: protoc.into(),
  }]
}
