// Copyright 2019 TiKV Project Authors. Licensed under Apache-2.0.
#[cfg(feature = "protobuf-codec")]
// Allow deprecated as TiKV pin versions to a outdated one.
fn generate_protobuf() {
  use std::io::Write;
  let customize = <protobuf_codegen::Customize as std::default::Default>::default();
  // Set the output directory for generated files
  let out_dir = std::env::var("OUT_DIR").unwrap();

  let mut cg = protobuf_codegen::Codegen::new();
  cg.pure();

  cg.inputs(["proto/profile.proto"]).includes(["proto"]);

  cg.customize(customize);
  cg.out_dir(&out_dir).run().unwrap();

  // Optionally, write a mod.rs file for module inclusion
  let mut f = std::fs::File::create(format!("{}/mod.rs", out_dir)).unwrap();
  write!(f, "pub mod profile;").unwrap();
}

#[cfg(all(feature = "prost-codec", not(feature = "protobuf-codec")))]
fn generate_prost() {
  use std::fmt::Write;
  use std::fs::File;
  use std::fs::{
    self,
  };
  use std::io::BufRead;
  use std::io::BufReader;
  use std::io::Read;

  use sha2::Digest;
  use sha2::Sha256;

  const PRE_GENERATED_PATH: &str = "proto/perftools.profiles.rs";

  // Calculate the SHA256 of the proto file
  let mut hasher = Sha256::new();
  let mut proto_file = File::open("proto/profile.proto").unwrap();
  let mut buffer = [0; 8192];
  loop {
    let bytes_read = proto_file.read(&mut buffer).unwrap();
    if bytes_read == 0 {
      break;
    }
    hasher.update(&buffer[..bytes_read]);
  }
  let mut hex = String::new();
  for b in hasher.finalize() {
    write!(&mut hex, "{:02x}", b).unwrap();
  }
  let hash_comment = format!("// {}  proto/profile.proto", hex);

  let first_line = File::open(PRE_GENERATED_PATH)
    .and_then(|f| {
      let mut reader = BufReader::new(f);
      let mut first_line = String::new();
      reader.read_line(&mut first_line)?;
      Ok(first_line)
    })
    .unwrap_or_default();
  // If the hash of the proto file changes, regenerate the prost file.
  if first_line.trim() != hash_comment {
    prost_build::Config::new()
      .out_dir("proto/")
      .compile_protos(&["proto/profile.proto"], &["proto/"])
      .unwrap();
    // Prepend the hash comment to the generated file.
    let generated = fs::read_to_string(PRE_GENERATED_PATH).unwrap();
    let with_hex = format!("{}\n\n{}", hash_comment, generated);
    fs::write(PRE_GENERATED_PATH, with_hex).unwrap();
  }
}

fn configure_backend_cfgs() {
  println!("cargo:rustc-check-cfg=cfg(pprof_framehop_backend)");
  println!("cargo:rustc-check-cfg=cfg(pprof_frame_pointer_backend)");

  let target_arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap();
  let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();
  let framehop_enabled = std::env::var_os("CARGO_FEATURE_FRAMEHOP_UNWINDER").is_some();
  let frame_pointer_enabled = std::env::var_os("CARGO_FEATURE_FRAME_POINTER").is_some();

  let framehop_supported = matches!(target_arch.as_str(), "x86_64" | "aarch64") && matches!(target_os.as_str(), "linux" | "macos");
  let frame_pointer_supported = matches!(target_arch.as_str(), "x86_64" | "aarch64" | "riscv64" | "loongarch64");

  if framehop_enabled && framehop_supported {
    println!("cargo:rustc-cfg=pprof_framehop_backend");
  } else if frame_pointer_enabled && frame_pointer_supported {
    println!("cargo:rustc-cfg=pprof_frame_pointer_backend");
  }
}

fn main() {
  println!("cargo:rerun-if-changed=proto/profile.proto");
  println!("cargo:rerun-if-changed=proto/perftools.profiles.rs");
  configure_backend_cfgs();

  #[cfg(all(feature = "prost-codec", not(feature = "protobuf-codec")))]
  generate_prost();
  #[cfg(feature = "protobuf-codec")]
  generate_protobuf();
}
