use anyhow::Context;

mod fcstarter;
mod booter;
mod virtual_sock;
mod ttrpc;
mod docker_ext4;
mod tap;
mod ateom;
mod atelet;
mod ateom_server;
mod http_server;
mod atlet_client;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // setup a process level default crypto provider  which is the ring 
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("failed to install rustls ring CryptoProvider");
    // start an http server for health check
    tokio::spawn(async move {
        http_server::start_http_server().await.expect("failed to start http server");
    });

    // start the grpc ateom server
    let pod_uuid = std::env::var("POD_UID").context("expect pod_uuid in the envrionment")?;
    let ateom_service = ateom_server::AteomService::new(pod_uuid);
    let unix_listener_stream = ateom_service.start_server().await.context("ateom server start failed")?;
    // now start the atlet grpc client
    tokio::spawn(async move {
        atlet_client::AtletClient::new().connect().await.expect("cannot send Work request to atlet");
    });

    ateom_service.start_incoming(unix_listener_stream).await.context("ateom server cannot start incoming")?;

    
    //
    // loop {
    //     std::future::pending::<()>().await
    // }
    Ok(())
}


