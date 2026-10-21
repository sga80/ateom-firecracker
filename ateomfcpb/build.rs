use ttrpc_codegen::Customize;

// Generates two independent copies of the bindings from the same api.proto:
// async (for ateom-fc, which is tokio-based throughout) and sync (for
// guest-init, which has no inherent need for async — see
// docs/dev/m2-plan.md for why sync is the better fit for PID 1 specifically).
fn main() {
    ttrpc_codegen::Codegen::new()
        .out_dir("src/async_protos")
        .inputs(&["src/protos/api.proto"])
        .include("src/protos")
        .rust_protobuf()
        .customize(Customize {
            async_all: true,
            gen_mod: true,
            ..Default::default()
        })
        .run()
        .expect("Cmd failed (async).");

    ttrpc_codegen::Codegen::new()
        .out_dir("src/sync_protos")
        .inputs(&["src/protos/api.proto"])
        .include("src/protos")
        .rust_protobuf()
        .customize(Customize {
            async_all: false,
            gen_mod: true,
            ..Default::default()
        })
        .run()
        .expect("Cmd failed (sync).");
}
