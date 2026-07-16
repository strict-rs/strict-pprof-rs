// Copyright 2019 TiKV Project Authors. Licensed under Apache-2.0.

use std::io::Write;

mod build_support;

use build_support::selected_backend_cfg;

#[derive(Debug, thiserror::Error)]
enum BuildError {
  #[error("environment variable {name} is unavailable: {source}")]
  Env {
    name:   &'static str,
    source: std::env::VarError,
  },
  #[error("{0}")]
  Io(#[from] std::io::Error),
  #[error("{0}")]
  Format(#[from] std::fmt::Error),
  #[error("protobuf generation failed: {0}")]
  #[cfg(feature = "protobuf-codec")]
  Protobuf(String),
}

fn env_var(name: &'static str) -> Result<String, BuildError> {
  std::env::var(name).map_err(|source| BuildError::Env {
    name,
    source,
  })
}

fn emit_cargo_directive(directive: &str) -> Result<(), BuildError> {
  let stdout = std::io::stdout();
  let mut stdout = stdout.lock();
  writeln!(stdout, "{directive}")?;
  Ok(())
}

#[cfg(feature = "protobuf-codec")]
fn generate_protobuf() -> Result<(), BuildError> {
  use std::path::PathBuf;

  let generated_dir = PathBuf::from(env_var("OUT_DIR")?).join("protobuf_generated");
  let mut codegen = protobuf_codegen::CodeGen::new();
  codegen.input("proto/profile.proto");
  codegen.include(".");
  codegen.output_dir(generated_dir);
  codegen.generate_and_compile().map_err(BuildError::Protobuf)?;
  Ok(())
}

#[cfg(all(feature = "prost-codec", not(feature = "protobuf-codec")))]
fn generate_prost() -> Result<(), BuildError> {
  use std::fmt::Write as _;
  use std::fs;
  use std::fs::File;
  use std::io::BufRead;
  use std::io::BufReader;
  use std::io::Read;

  use sha2::Digest;
  use sha2::Sha256;

  const PRE_GENERATED_PATH: &str = "proto/perftools.profiles.rs";
  const PROST_GENERATOR_CONFIG: &str = "prost-build-0.14-eq-aggregates-v2";

  let mut hasher = Sha256::new();
  let mut proto_file = File::open("proto/profile.proto")?;
  let mut buffer = [0; 8192];
  loop {
    let bytes_read = proto_file.read(&mut buffer)?;
    if bytes_read == 0 {
      break;
    }
    hasher.update(&buffer[..bytes_read]);
  }

  let mut hex = String::new();
  for byte in hasher.finalize() {
    write!(&mut hex, "{byte:02x}")?;
  }
  let hash_comment = format!("// {hex}  proto/profile.proto  {PROST_GENERATOR_CONFIG}");

  let first_line = File::open(PRE_GENERATED_PATH)
    .and_then(|file| {
      let mut reader = BufReader::new(file);
      let mut first_line = String::new();
      reader.read_line(&mut first_line)?;
      Ok(first_line)
    })
    .unwrap_or_default();

  if first_line.trim() == hash_comment {
    return Ok(());
  }

  let mut config = prost_build::Config::new();
  config.out_dir("proto/");
  config.type_attribute("perftools.profiles.Profile", "#[derive(Eq)]");
  config.type_attribute("perftools.profiles.Sample", "#[derive(Eq)]");
  config.type_attribute("perftools.profiles.Location", "#[derive(Eq)]");
  config.compile_protos(&["proto/profile.proto"], &["proto/"])?;

  let generated = fs::read_to_string(PRE_GENERATED_PATH)?;
  let with_hex = format!("{hash_comment}\n\n{generated}");
  fs::write(PRE_GENERATED_PATH, with_hex)?;
  Ok(())
}

fn configure_backend_cfgs() -> Result<(), BuildError> {
  emit_cargo_directive("cargo:rustc-check-cfg=cfg(pprof_framehop_backend)")?;
  emit_cargo_directive("cargo:rustc-check-cfg=cfg(pprof_frame_pointer_backend)")?;

  let target_arch = env_var("CARGO_CFG_TARGET_ARCH")?;
  let target_os = env_var("CARGO_CFG_TARGET_OS")?;
  let framehop_enabled = std::env::var_os("CARGO_FEATURE_FRAMEHOP_UNWINDER").is_some();
  let frame_pointer_enabled = std::env::var_os("CARGO_FEATURE_FRAME_POINTER").is_some();

  if let Some(backend_cfg) = selected_backend_cfg(&target_arch, &target_os, framehop_enabled, frame_pointer_enabled) {
    emit_cargo_directive(backend_cfg.cargo_cfg_directive())?;
  }

  Ok(())
}

fn main() -> Result<(), BuildError> {
  emit_cargo_directive("cargo:rerun-if-changed=proto/profile.proto")?;
  emit_cargo_directive("cargo:rerun-if-changed=proto/perftools.profiles.rs")?;
  emit_cargo_directive("cargo:rerun-if-env-changed=PROTOC")?;
  configure_backend_cfgs()?;

  #[cfg(all(feature = "prost-codec", not(feature = "protobuf-codec")))]
  generate_prost()?;
  #[cfg(feature = "protobuf-codec")]
  generate_protobuf()?;

  Ok(())
}
