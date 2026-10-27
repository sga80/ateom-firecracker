// Generates async tonic code (ateom-fc is tokio-based throughout, same
// reasoning as ateomfcpb's async build for the vsock protocol) for the real
// control-plane APIs, vendored at proto/ateom.proto and proto/atelet.proto.
//
// Uses protox (a pure-Rust proto compiler) instead of prost-build's default
// protoc-shelling-out, since neither this machine nor the Lima dev VM has a
// system `protoc` installed, and ateomfcpb's own build.rs already avoids
// that same dependency for the vsock protocol via ttrpc-codegen's
// rust_protobuf() mode.
fn main() -> std::io::Result<()> {
    // ateom.proto: the Ateom service ateom-fc itself serves, so only a
    // server is generated.
    //
    // tonic-prost-build's out_dir must already exist, so it's created here
    // rather than relying on a prior build to have left it behind.
    std::fs::create_dir_all("src/ateom_pb")?;
    let ateom_fds = protox::compile(["proto/ateom.proto"], ["proto"])
        .expect("failed to compile proto/ateom.proto");
    tonic_prost_build::configure()
        .build_server(true)
        .build_client(false)
        .out_dir("src/ateom_pb")
        .compile_fds(ateom_fds)?;

    // atelet.proto: the AteomSupport service atelet serves, which ateom-fc
    // calls as a client (e.g. RegisterWorker to report capacity) -- so only a
    // client is generated here, the opposite of ateom.proto above.
    std::fs::create_dir_all("src/atelet_pb")?;
    let atelet_fds = protox::compile(["proto/atelet.proto"], ["proto"])
        .expect("failed to compile proto/atelet.proto");
    tonic_prost_build::configure()
        .build_server(false)
        .build_client(true)
        .out_dir("src/atelet_pb")
        .compile_fds(atelet_fds)
}
