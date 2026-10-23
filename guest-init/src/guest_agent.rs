use crate::virtual_fs::MERGED_FINAL_DIR;
use anyhow::bail;
use ateomfcpb::sync_protos::api::{ExecRequest, ExecResponse, PingRequest, PingResponse};
use ateomfcpb::sync_protos::api_ttrpc::{create_guest_agent, GuestAgent};
use std::process::Command;
use std::sync::Arc;
use ttrpc::sync::{Server, TtrpcContext};

struct GuestAgentServer;

impl GuestAgent for GuestAgentServer {
    fn ping(&self, _ctx: &TtrpcContext, req: PingRequest) -> ttrpc::Result<PingResponse> {
        println!("received ping in guest init: {}", req.message);
        let mut resp = PingResponse::new();
        resp.message = "PONG".to_string();
        Ok(resp)
    }
    fn exec(&self, _ctx: &TtrpcContext, req: ExecRequest) -> ttrpc::Result<ExecResponse> {
        let exec_command = format!("{}{}", MERGED_FINAL_DIR, req.command);
        println!("executing command {}", exec_command);
        let child = Command::new(exec_command)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .spawn();

        let mut resp = ExecResponse::new();
        if child.is_err() {
            println!("failed to launch command");
            resp.success = false;
        } else {
            let mut child = child.unwrap();
            println!("[PID 1] Launched echo with PID: {}", child.id());
            // we are not going to wait here as the process needs to run. We will eventually write a reaper that reaps the child
            resp.success = true;
        }

        Ok(resp)
    }
}

#[cfg(target_os = "linux")]
pub fn get_ttrpc_server() -> anyhow::Result<Server> {
    println!("binding vsock at port 8080");
    // VMADDR_CID_ANY ("-1") listens on all available CIDs on port 8080 — same
    // address format as the async side, vsock parsing is shared code in ttrpc.
    let listener = Server::new().bind("vsock://-1:8080");
    if listener.is_err() {
        bail!("failed in setting listener")
    }
    let methods = create_guest_agent(Arc::new(GuestAgentServer));
    println!("created guest agent with methods");
    let server = listener.unwrap().register_service(methods);
    Ok(server)
}
