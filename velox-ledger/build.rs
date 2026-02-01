use std::io::Result;

fn main() -> Result<()> {
    // Set PROTOC environment variable to use bundled protoc from protobuf-src
    std::env::set_var("PROTOC", protobuf_src::protoc());

    // Compile the protobuf definitions
    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(
            &["proto/ledger.proto"],
            &["proto/"],
        )?;

    // Re-run if proto files change
    println!("cargo:rerun-if-changed=proto/ledger.proto");
    println!("cargo:rerun-if-changed=proto/");

    Ok(())
}
