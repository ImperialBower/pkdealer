use std::{env, error::Error, path::PathBuf};

fn main() -> Result<(), Box<dyn Error>> {
    let protoc_path = protoc_bin_vendored::protoc_bin_path()?;
    // Point tonic/prost to a vendored protoc binary to avoid host toolchain drift.
    // SAFETY: Build scripts run as a single process for this crate and set this
    // variable before any worker threads are created.
    unsafe {
        env::set_var("PROTOC", protoc_path);
    }

    let proto_dir = PathBuf::from("proto");
    let proto_file = proto_dir.join("dealer.proto");

    println!("cargo:rerun-if-changed={}", proto_file.display());

    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(&[proto_file], &[proto_dir])?;

    Ok(())
}
