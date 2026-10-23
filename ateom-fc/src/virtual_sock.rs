use anyhow::{bail, Result};
use ateomfcpb::async_protos::api::{ExecRequest, PingRequest};
use ateomfcpb::async_protos::api_ttrpc::GuestAgentClient;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use ttrpc::context::Context;
use ttrpc::r#async::transport::Socket;
use ttrpc::r#async::Client as TtrpcClient;

const CONNECT_RETRIES: u32 = 10;
const CONNECT_RETRY_INTERVAL: Duration = Duration::from_secs(1);


pub async fn start_connect(uds_path: String) -> Result<()> {
    let agent = connect_with_retries(&uds_path).await?;
    run_conversation(&agent).await
}

// Retries only the connection handshake — this is the part that's actually
// racing the guest's own boot (see the module comment history / m1 doc).
// Ping/Exec happen once a connection is established and are not retried
// here: an application-level failure (e.g. Exec reporting a non-zero exit)
// is a real failure, not a reason to reconnect and redo the whole
// conversation from scratch.
async fn connect_with_retries(uds_path: &str) -> Result<GuestAgentClient> {
    for _ in 0..CONNECT_RETRIES {
        match try_connect(uds_path).await {
            Ok(agent) => return Ok(agent),
            Err(err) => {
                eprintln!("guest connect attempt failed: {err}");
                tokio::time::sleep(CONNECT_RETRY_INTERVAL).await;
            }
        }
    }
    bail!("could not connect to guest over vsock after {CONNECT_RETRIES} attempts")
}

async fn try_connect(uds_path: &str) -> Result<GuestAgentClient> {
    // the uds_path is the path to the unix domain socket which is set by vsock.rs
    let mut stream = UnixStream::connect(uds_path).await?;

    stream.write_all(b"CONNECT 8080\n").await?;

    // firecracker will respond back first with OK port
    let mut buffer = [0u8; 1024];
    let n = stream.read(&mut buffer).await?;
    let fc_response = String::from_utf8_lossy(&buffer[..n]);
    if !fc_response.starts_with("OK") {
        bail!("invalid response from firecracker: {fc_response:?}");
    }

    let socket: Socket = stream.into(); // this is possible because ttrpc's Socket implements tokio UnixStream
    let ttrpc_client = TtrpcClient::new(socket);
    Ok(GuestAgentClient::new(ttrpc_client))
}

async fn run_conversation(agent: &GuestAgentClient) -> Result<()> {
    let mut ping_request = PingRequest::new();
    ping_request.message = "PING".to_string();
    let ping_response = agent.ping(Context::default(), &ping_request).await?;
    println!("response from init is {}", ping_response.message);
    if ping_response.message.trim() != "PONG" {
        bail!("invalid response from guest: {:?}", ping_response.message);
    }

    // now send the command to execute
    let mut exec_request = ExecRequest::new();
    exec_request.command = String::from("/counter"); // hard coded to the counter example for now.
    let exec_response = agent.exec(Context::default(), &exec_request).await?;
    if exec_response.success {
        println!("command executed successfully")
    } else {
        println!("command failed to execute")
    }


    Ok(())
}