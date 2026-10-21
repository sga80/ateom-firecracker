mod fcstarter;
mod booter;
mod virtual_sock;
mod ttrpc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let client = get_reqwest_http_client()?;
    fcstarter::start(client.clone()).await?;

    let kernel_path = "/var/lib/ateomfc/vmlinux";
    let root_fs_path = "/var/lib/ateomfc/rootfs.ext4";
    let mut booter = booter::Booter::new(client.clone(), String::from(kernel_path), String::from(root_fs_path));
    booter.boot().await?;
    println!("booter started, starting virtual sock now ");
    // now connect to the af_sock async
    virtual_sock::start_connect(booter.get_uds_path()).await?;

    Ok(())
}

fn get_reqwest_http_client() -> anyhow::Result<reqwest::Client> {
    let client = reqwest::Client::builder()
        .unix_socket(fcstarter::SOCKET_PATH)
        .build()?;
    Ok(client)
}
