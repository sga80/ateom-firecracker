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
        println!("received command to execute");
        let child = Command::new("/bin/echo")
            .arg("hello world")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .spawn();

        let mut resp = ExecResponse::new();
        if child.is_err() {
            println!("failed to launch echo");
            resp.exit_code = -1;
        } else {
            let mut child = child.unwrap();
            println!("[PID 1] Launched echo with PID: {}", child.id());
            let result = child.wait().unwrap(); // the wait call reaps the process so there is no additional work required here 
            resp.exit_code = result.code().unwrap();
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
