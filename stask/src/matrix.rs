//! Local matrix extension command: `just x matrix`.

use std::path::Path;

use bpaf::OptionParser;
use bpaf::Parser as _;
use bpaf::construct;
use bpaf::long;
use strict_standard::EnvironmentChange;
use strict_standard::OutputPolicy;
use template_core::cli::context::CommandContext;
use template_core::cli::extension::ExtensionCommand;
use template_core::cli::extension::extension_command;
use template_core::cli::output::StatusKind;
use template_core::cli::output::plain;
use template_core::sys::process::ToolColor;
use template_core::sys::process::require_success;

use crate::extensions::ProjectCommand;
use crate::proto;
use crate::workspace;

/// Directory used for example-generated `profile.pb` output.
const EXAMPLE_OUTPUT_DIR: &str = "target/pprof-matrix/examples";

/// Parsed arguments for `just x matrix`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Args {
  /// Selected matrix mode.
  mode: MatrixMode,
}

/// Matrix mode selected by the command-line flags.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MatrixMode {
  /// Run every matrix group.
  Full,
  /// Run focused codec build checks.
  Focused,
  /// Run aggregate check, clippy, and tests.
  Aggregate,
  /// Run protobuf/prost example binaries.
  Examples,
  /// Run dependency hygiene checks.
  Deps,
}

/// Directory to use while running a matrix step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StepDirectory {
  /// Leave the process in its current directory.
  Current,
  /// Run from the isolated example output directory.
  ExampleOutput,
}

/// One external command in the matrix.
#[derive(Clone, Debug, Eq, PartialEq)]
struct CommandStep {
  /// Human-facing step label.
  label:        &'static str,
  /// Program to execute.
  program:      &'static str,
  /// Arguments passed to the program.
  args:         Vec<String>,
  /// Color integration strategy for the child.
  color:        ToolColor,
  /// Whether this step needs the pinned `PROTOC` environment binding.
  needs_protoc: bool,
  /// Directory policy for the step.
  directory:    StepDirectory,
}

/// Declare the command's `bpaf` options.
#[allow(
  clippy::single_call_fn,
  reason = "a named options() keeps the extension parser beside the matrix command it configures"
)]
fn options() -> OptionParser<Args> {
  let focused = long("focused")
    .help("Run focused codec build checks")
    .req_flag(MatrixMode::Focused);
  let aggregate = long("aggregate")
    .help("Run aggregate check, clippy, and tests")
    .req_flag(MatrixMode::Aggregate);
  let examples = long("examples")
    .help("Run protobuf example binaries")
    .req_flag(MatrixMode::Examples);
  let deps = long("deps").help("Run dependency hygiene checks").req_flag(MatrixMode::Deps);

  construct!([focused, aggregate, examples, deps])
    .fallback(MatrixMode::Full)
    .map(|mode| Args {
      mode,
    })
    .to_options()
    .descr("Run the pprof protobuf/prost verification matrix")
}

/// Run the matrix extension command.
///
/// # Errors
///
/// Returns a workflow error when the workspace root cannot be resolved or the
/// pinned `protoc` version is not available, and returns process errors from
/// any failed matrix step.
#[allow(
  clippy::single_call_fn,
  reason = "extension dispatch calls the handler once through ProjectCommand::run"
)]
pub(crate) fn execute(context: &CommandContext, args: Args) -> template_core::Result<()> {
  let root = workspace::root()?;
  let steps = steps_for_mode(args.mode, &root);
  run_steps(context, &root, &steps)
}

/// Registry entry point referenced from `stask/src/extensions.rs`.
#[must_use]
#[allow(
  clippy::single_call_fn,
  reason = "the command() seam is consumed once by the local extension registry"
)]
pub(crate) fn command() -> template_core::Result<ExtensionCommand<ProjectCommand>> {
  extension_command(
    "matrix",
    "Run the protobuf/prost verification matrix",
    options(),
    ProjectCommand::Matrix,
  )
}

/// Run every selected step, resolving `protoc` once when required.
#[allow(
  clippy::single_call_fn,
  reason = "the matrix runner owns the setup-once contract before iterating selected command steps"
)]
fn run_steps(context: &CommandContext, root: &Path, steps: &[CommandStep]) -> template_core::Result<()> {
  let toolchain = if steps_require_protoc(steps) {
    Some(proto::setup_toolchain(context)?)
  } else {
    None
  };

  for step in steps {
    run_step(context, root, step, toolchain.as_ref())?;
  }
  Ok(())
}

/// `true` when at least one step needs the pinned compiler.
#[allow(
  clippy::single_call_fn,
  reason = "the setup predicate names why the toolchain is resolved once for a whole matrix run"
)]
fn steps_require_protoc(steps: &[CommandStep]) -> bool {
  steps.iter().any(|step| step.needs_protoc)
}

/// Run one matrix step in its configured directory.
#[allow(
  clippy::single_call_fn,
  reason = "one step combines status output, directory policy, and process execution as a named unit"
)]
fn run_step(context: &CommandContext, root: &Path, step: &CommandStep, toolchain: Option<&proto::Toolchain>) -> template_core::Result<()> {
  context.status(StatusKind::Info, plain(format!("matrix: {}", step.label)))?;
  let environment = if step.needs_protoc {
    let Some(validated_toolchain) = toolchain else {
      return Err(proto::workflow_error(format!(
        "matrix step `{}` requires PROTOC before setup",
        step.label
      )));
    };
    vec![EnvironmentChange::Set {
      name:  "PROTOC".into(),
      value: validated_toolchain.protoc().into(),
    }]
  } else {
    Vec::new()
  };
  let mut request = context.process_request(
    step.program,
    &step.args,
    step.color,
    OutputPolicy::Inherit,
    OutputPolicy::Inherit,
    &environment,
  );
  if step.directory == StepDirectory::ExampleOutput {
    let output_dir = root.join(EXAMPLE_OUTPUT_DIR);
    context.file_system().create_dir_all(&output_dir)?;
    request.current_dir = Some(output_dir);
  }
  let output = context.execute_process(&request)?;
  drop(require_success(&request, output)?);
  Ok(())
}

/// Build the selected matrix steps.
#[allow(
  clippy::single_call_fn,
  reason = "mode-to-step selection is the matrix command's core dispatch table and is tested directly"
)]
fn steps_for_mode(mode: MatrixMode, root: &Path) -> Vec<CommandStep> {
  match mode {
    MatrixMode::Full => full_steps(root),
    MatrixMode::Focused => focused_steps(root),
    MatrixMode::Aggregate => aggregate_steps(root),
    MatrixMode::Examples => example_steps(root),
    MatrixMode::Deps => dependency_steps(root),
  }
}

/// Build the full matrix step list.
#[allow(
  clippy::single_call_fn,
  reason = "full mode is the user-facing default and deserves a named composition order"
)]
fn full_steps(root: &Path) -> Vec<CommandStep> {
  let mut steps = focused_steps(root);
  steps.extend(aggregate_steps(root));
  steps.extend(example_steps(root));
  steps.extend(dependency_steps(root));
  steps
}

/// Build focused codec build checks.
fn focused_steps(root: &Path) -> Vec<CommandStep> {
  vec![
    cargo_step(
      "check protobuf-codec",
      "check",
      root,
      &["--features", "protobuf-codec", "--locked"],
      true,
      StepDirectory::Current,
    ),
    cargo_step(
      "check prost-codec",
      "check",
      root,
      &["--features", "prost-codec", "--locked"],
      true,
      StepDirectory::Current,
    ),
    cargo_step(
      "check flamegraph protobuf-codec targets",
      "check",
      root,
      &["--features", "flamegraph,protobuf-codec", "--all-targets", "--locked"],
      true,
      StepDirectory::Current,
    ),
    cargo_step(
      "check flamegraph prost-codec targets",
      "check",
      root,
      &["--features", "flamegraph,prost-codec", "--all-targets", "--locked"],
      true,
      StepDirectory::Current,
    ),
  ]
}

/// Build aggregate workspace checks.
fn aggregate_steps(root: &Path) -> Vec<CommandStep> {
  vec![
    cargo_step(
      "check workspace all features",
      "check",
      root,
      &["--workspace", "--all-features", "--all-targets", "--locked"],
      true,
      StepDirectory::Current,
    ),
    cargo_step(
      "clippy workspace all features",
      "clippy",
      root,
      &["--workspace", "--all-features", "--all-targets", "--", "-D", "warnings"],
      true,
      StepDirectory::Current,
    ),
    cargo_step(
      "test workspace all targets",
      "test",
      root,
      &["--all-targets", "--workspace"],
      true,
      StepDirectory::Current,
    ),
  ]
}

/// Build protobuf example runs.
fn example_steps(root: &Path) -> Vec<CommandStep> {
  vec![
    cargo_step(
      "run protobuf-codec example",
      "run",
      root,
      &["--example", "profile_proto_with_protobuf_codec", "--features", "protobuf-codec"],
      true,
      StepDirectory::ExampleOutput,
    ),
    cargo_step(
      "run prost-codec example",
      "run",
      root,
      &["--example", "profile_proto_with_prost", "--features", "prost-codec"],
      true,
      StepDirectory::ExampleOutput,
    ),
  ]
}

/// Build dependency hygiene checks.
fn dependency_steps(root: &Path) -> Vec<CommandStep> {
  vec![
    cargo_step(
      "check duplicate dependencies",
      "tree",
      root,
      &[
        "--duplicates", "--locked", "--target", "all", "--all-features", "--no-default-features",
      ],
      false,
      StepDirectory::Current,
    ),
    CommandStep {
      label:        "run supply-chain audit",
      program:      "just",
      args:         vec!["audit".to_owned()],
      color:        ToolColor::EnvOnly,
      needs_protoc: false,
      directory:    StepDirectory::Current,
    },
  ]
}

/// Build one Cargo command step.
fn cargo_step(
  label: &'static str,
  subcommand: &'static str,
  root: &Path,
  args: &[&str],
  needs_protoc: bool,
  directory: StepDirectory,
) -> CommandStep {
  CommandStep {
    label,
    program: "cargo",
    args: cargo_args(subcommand, root, args),
    color: ToolColor::CargoGlobal,
    needs_protoc,
    directory,
  }
}

/// Build Cargo arguments with an absolute manifest path.
#[allow(
  clippy::single_call_fn,
  reason = "all matrix Cargo steps share the same manifest anchoring contract"
)]
fn cargo_args(subcommand: &str, root: &Path, args: &[&str]) -> Vec<String> {
  let mut owned = vec![
    subcommand.to_owned(),
    "--manifest-path".to_owned(),
    root.join("Cargo.toml").display().to_string(),
  ];
  owned.extend(args.iter().map(|arg| (*arg).to_owned()));
  owned
}

#[cfg(test)]
mod tests {
  use strict_standard::OutputPolicy;
  use strict_test_support::EffectEvent;
  use strict_test_support::TestFailure;
  use strict_test_support::ensure;
  use strict_test_support::ensure_ok;
  use strict_test_support::ensure_some;
  use template_core::CoreError;
  use template_core::sys::process::ToolColor;

  use super::Args;
  use super::EXAMPLE_OUTPUT_DIR;
  use super::MatrixMode;
  use super::StepDirectory;
  use super::dependency_steps;
  use super::execute;
  use super::options;
  use super::steps_for_mode;
  use crate::proto::EXPECTED_PROTOC_VERSION;
  use crate::test_support::ProcessFixture;
  use crate::test_support::expected_setup_requests;
  use crate::test_support::extension_request;
  use crate::test_support::protoc_environment;
  use crate::test_support::recording_context;
  use crate::workspace::root as workspace_root;

  #[test]
  fn parser_defaults_to_full_matrix() -> Result<(), TestFailure> {
    let args = parse_args(&[])?;
    ensure(args.mode == MatrixMode::Full, "matrix without flags must select the full mode")
  }

  #[test]
  fn parser_selects_each_single_mode_flag() -> Result<(), TestFailure> {
    let cases = [
      (["--focused"].as_slice(), MatrixMode::Focused),
      (["--aggregate"].as_slice(), MatrixMode::Aggregate),
      (["--examples"].as_slice(), MatrixMode::Examples),
      (["--deps"].as_slice(), MatrixMode::Deps),
    ];

    for (argv, expected) in cases {
      let args = parse_args(argv)?;
      ensure(args.mode == expected, "each matrix flag must select its matching mode")?;
    }
    Ok(())
  }

  #[test]
  fn parser_rejects_multiple_or_unknown_mode_flags() -> Result<(), TestFailure> {
    ensure(
      options().run_inner(&["--focused", "--deps"]).is_err(),
      "matrix parser must reject multiple mode flags",
    )?;
    ensure(
      options().run_inner(&["--everything"]).is_err(),
      "matrix parser must reject unknown flags",
    )
  }

  #[test]
  fn mode_step_selection_matches_the_named_groups() -> Result<(), TestFailure> {
    ensure(
      step_labels(MatrixMode::Focused)?
        == [
          "check protobuf-codec",
          "check prost-codec",
          "check flamegraph protobuf-codec targets",
          "check flamegraph prost-codec targets",
        ],
      "focused mode must contain only focused codec checks",
    )?;
    ensure(
      step_labels(MatrixMode::Aggregate)?
        == [
          "check workspace all features",
          "clippy workspace all features",
          "test workspace all targets",
        ],
      "aggregate mode must contain check, clippy, and tests",
    )?;
    ensure(
      step_labels(MatrixMode::Examples)? == ["run protobuf-codec example", "run prost-codec example"],
      "examples mode must contain both protobuf example binaries",
    )?;
    ensure(
      step_labels(MatrixMode::Deps)? == ["check duplicate dependencies", "run supply-chain audit"],
      "deps mode must contain dependency hygiene checks",
    )
  }

  #[test]
  fn full_mode_composes_groups_in_order() -> Result<(), TestFailure> {
    let labels = step_labels(MatrixMode::Full)?;
    ensure(
      labels
        == [
          "check protobuf-codec",
          "check prost-codec",
          "check flamegraph protobuf-codec targets",
          "check flamegraph prost-codec targets",
          "check workspace all features",
          "clippy workspace all features",
          "test workspace all targets",
          "run protobuf-codec example",
          "run prost-codec example",
          "check duplicate dependencies",
          "run supply-chain audit",
        ],
      "full mode must run focused, aggregate, examples, then deps",
    )
  }

  #[test]
  fn dependency_steps_do_not_require_protoc() -> Result<(), TestFailure> {
    let root = ensure_ok(workspace_root(), "workspace root must resolve")?;
    let steps = dependency_steps(&root);
    ensure(
      steps.iter().all(|step| !step.needs_protoc),
      "dependency hygiene steps must not require protoc",
    )
  }

  #[test]
  fn focused_execution_records_proto_setup_then_codec_checks() -> Result<(), TestFailure> {
    let fixtures = [
      ProcessFixture::Success,
      ProcessFixture::Stdout("/tool/protoc\n"),
      ProcessFixture::Stdout("libprotoc 35.1\n"),
      ProcessFixture::Success,
      ProcessFixture::Success,
      ProcessFixture::Success,
      ProcessFixture::Success,
    ];
    let (context, recorder) = recording_context(&fixtures)?;
    ensure_ok(
      execute(&context, Args {
        mode: MatrixMode::Focused
      }),
      "focused matrix execution should succeed with recorded tools",
    )?;

    let manifest = manifest_path()?;
    let protoc_environment = protoc_environment("/tool/protoc");
    let mut expected = expected_setup_requests(&context, "/tool/protoc");
    expected.extend([
      extension_request(
        &context,
        "cargo",
        &[
          "check", "--manifest-path", &manifest, "--features", "protobuf-codec", "--locked",
        ],
        ToolColor::CargoGlobal,
        OutputPolicy::Inherit,
        &protoc_environment,
      ),
      extension_request(
        &context,
        "cargo",
        &["check", "--manifest-path", &manifest, "--features", "prost-codec", "--locked"],
        ToolColor::CargoGlobal,
        OutputPolicy::Inherit,
        &protoc_environment,
      ),
      extension_request(
        &context,
        "cargo",
        &[
          "check", "--manifest-path", &manifest, "--features", "flamegraph,protobuf-codec", "--all-targets", "--locked",
        ],
        ToolColor::CargoGlobal,
        OutputPolicy::Inherit,
        &protoc_environment,
      ),
      extension_request(
        &context,
        "cargo",
        &[
          "check", "--manifest-path", &manifest, "--features", "flamegraph,prost-codec", "--all-targets", "--locked",
        ],
        ToolColor::CargoGlobal,
        OutputPolicy::Inherit,
        &protoc_environment,
      ),
    ]);
    ensure(
      recorder.process_requests() == expected,
      "focused mode must record setup followed by the four codec checks",
    )
  }

  #[test]
  fn deps_execution_records_dependency_commands_without_proto_setup() -> Result<(), TestFailure> {
    let fixtures = [ProcessFixture::Success, ProcessFixture::Success];
    let (context, recorder) = recording_context(&fixtures)?;
    ensure_ok(
      execute(&context, Args {
        mode: MatrixMode::Deps
      }),
      "deps matrix execution should succeed with recorded tools",
    )?;

    let manifest = manifest_path()?;
    ensure(
      recorder.process_requests()
        == [
          extension_request(
            &context,
            "cargo",
            &[
              "tree", "--manifest-path", &manifest, "--duplicates", "--locked", "--target", "all", "--all-features",
              "--no-default-features",
            ],
            ToolColor::CargoGlobal,
            OutputPolicy::Inherit,
            &[],
          ),
          extension_request(&context, "just", &["audit"], ToolColor::EnvOnly, OutputPolicy::Inherit, &[]),
        ],
      "deps mode must run cargo tree and just audit without proto setup",
    )
  }

  #[test]
  fn wrong_protoc_version_stops_before_cargo() -> Result<(), TestFailure> {
    let fixtures = [
      ProcessFixture::Success,
      ProcessFixture::Stdout("/tool/protoc\n"),
      ProcessFixture::Stdout("libprotoc 3.21.12\n"),
    ];
    let (context, recorder) = recording_context(&fixtures)?;
    let error = ensure_some(
      execute(&context, Args {
        mode: MatrixMode::Focused
      })
      .err(),
      "wrong protoc version should fail the matrix",
    )?;
    let rendered = error.to_string();
    ensure(
      matches!(error, CoreError::WorkflowFailure { .. }),
      "wrong protoc version must surface a workflow-owned error",
    )?;
    ensure(
      rendered.contains(EXPECTED_PROTOC_VERSION),
      "wrong protoc version must name the expected version",
    )?;
    ensure(
      recorder.process_requests() == expected_setup_requests(&context, "/tool/protoc"),
      "wrong protoc version must stop before running Cargo",
    )
  }

  #[test]
  fn failing_matrix_step_stops_later_steps() -> Result<(), TestFailure> {
    let fixtures = [
      ProcessFixture::Success,
      ProcessFixture::Stdout("/tool/protoc\n"),
      ProcessFixture::Stdout("libprotoc 35.1\n"),
      ProcessFixture::Failure,
    ];
    let (context, recorder) = recording_context(&fixtures)?;
    ensure(
      execute(&context, Args {
        mode: MatrixMode::Focused
      })
      .is_err(),
      "a failing matrix step should fail execution",
    )?;
    let manifest = manifest_path()?;
    let protoc_environment = protoc_environment("/tool/protoc");
    let mut expected = expected_setup_requests(&context, "/tool/protoc");
    expected.push(extension_request(
      &context,
      "cargo",
      &[
        "check", "--manifest-path", &manifest, "--features", "protobuf-codec", "--locked",
      ],
      ToolColor::CargoGlobal,
      OutputPolicy::Inherit,
      &protoc_environment,
    ));
    ensure(
      recorder.process_requests() == expected,
      "failing the first Cargo check must stop before later focused checks",
    )
  }

  #[test]
  fn examples_use_the_isolated_directory_and_stop_after_failure() -> Result<(), TestFailure> {
    let success_fixtures = [
      ProcessFixture::Success,
      ProcessFixture::Stdout("/tool/protoc\n"),
      ProcessFixture::Stdout("libprotoc 35.1\n"),
      ProcessFixture::Success,
      ProcessFixture::Success,
    ];
    let (success_context, success_recorder) = recording_context(&success_fixtures)?;
    ensure_ok(
      execute(&success_context, Args {
        mode: MatrixMode::Examples,
      }),
      "examples mode should succeed with recorded commands",
    )?;

    let root = ensure_ok(workspace_root(), "workspace root must resolve")?;
    let output_dir = root.join(EXAMPLE_OUTPUT_DIR);
    let manifest = manifest_path()?;
    let protoc_environment = protoc_environment("/tool/protoc");
    let mut protobuf_example = extension_request(
      &success_context,
      "cargo",
      &[
        "run",
        "--manifest-path",
        &manifest,
        "--example",
        "profile_proto_with_protobuf_codec",
        "--features",
        "protobuf-codec",
      ],
      ToolColor::CargoGlobal,
      OutputPolicy::Inherit,
      &protoc_environment,
    );
    protobuf_example.current_dir = Some(output_dir.clone());
    let mut prost_example = extension_request(
      &success_context,
      "cargo",
      &[
        "run", "--manifest-path", &manifest, "--example", "profile_proto_with_prost", "--features", "prost-codec",
      ],
      ToolColor::CargoGlobal,
      OutputPolicy::Inherit,
      &protoc_environment,
    );
    prost_example.current_dir = Some(output_dir.clone());
    let setup_requests = expected_setup_requests(&success_context, "/tool/protoc");
    let mut expected_events = setup_requests
      .into_iter()
      .map(EffectEvent::Process)
      .collect::<Vec<EffectEvent>>();
    expected_events.extend([
      EffectEvent::CreateDirAll(output_dir.clone()),
      EffectEvent::Process(protobuf_example.clone()),
      EffectEvent::CreateDirAll(output_dir.clone()),
      EffectEvent::Process(prost_example),
    ]);
    ensure(
      success_recorder.events() == expected_events,
      "examples mode must create the output directory before each isolated example run",
    )?;

    let failure_fixtures = [
      ProcessFixture::Success,
      ProcessFixture::Stdout("/tool/protoc\n"),
      ProcessFixture::Stdout("libprotoc 35.1\n"),
      ProcessFixture::Failure,
    ];
    let (failure_context, failure_recorder) = recording_context(&failure_fixtures)?;
    ensure(
      execute(&failure_context, Args {
        mode: MatrixMode::Examples,
      })
      .is_err(),
      "examples mode should surface a failing recorded example command",
    )?;
    let failure_setup = expected_setup_requests(&failure_context, "/tool/protoc");
    let mut failure_events = failure_setup
      .into_iter()
      .map(EffectEvent::Process)
      .collect::<Vec<EffectEvent>>();
    let mut failed_example = extension_request(
      &failure_context,
      "cargo",
      &[
        "run",
        "--manifest-path",
        &manifest,
        "--example",
        "profile_proto_with_protobuf_codec",
        "--features",
        "protobuf-codec",
      ],
      ToolColor::CargoGlobal,
      OutputPolicy::Inherit,
      &protoc_environment,
    );
    failed_example.current_dir = Some(output_dir.clone());
    failure_events.extend([EffectEvent::CreateDirAll(output_dir), EffectEvent::Process(failed_example)]);
    ensure(
      failure_recorder.events() == failure_events,
      "a failed first example must prevent the second directory preparation and process request",
    )
  }

  fn parse_args(argv: &[&str]) -> Result<Args, TestFailure> {
    ensure_some(options().run_inner(argv).ok(), "matrix arguments must parse")
  }

  fn step_labels(mode: MatrixMode) -> Result<Vec<&'static str>, TestFailure> {
    let root = ensure_ok(workspace_root(), "workspace root must resolve")?;
    Ok(steps_for_mode(mode, &root).iter().map(|step| step.label).collect())
  }

  fn manifest_path() -> Result<String, TestFailure> {
    let root = ensure_ok(workspace_root(), "workspace root must resolve")?;
    Ok(root.join("Cargo.toml").display().to_string())
  }

  #[test]
  fn example_steps_use_the_isolated_output_directory() -> Result<(), TestFailure> {
    let root = ensure_ok(workspace_root(), "workspace root must resolve")?;
    ensure(
      steps_for_mode(MatrixMode::Examples, &root)
        .iter()
        .all(|step| step.directory == StepDirectory::ExampleOutput),
      "all example steps must run from the isolated output directory",
    )
  }
}
