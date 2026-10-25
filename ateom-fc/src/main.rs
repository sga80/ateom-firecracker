use std::time::Instant;
use uuid::Uuid;

mod fcstarter;
mod booter;
mod virtual_sock;
mod ttrpc;
mod docker_ext4;
mod tap;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // first setup the docker ext4
    let actor_uuid = Uuid::new_v4();
    let docker_image_ext4_path = docker_ext4::docker_lower_ext4_block_device("/var/lib/ateomfc/test-images/counter-rootfs")?;
    let docker_image_rw_upper_path = docker_ext4::docker_upper_ext4_block_device(actor_uuid)?;
    let client = get_reqwest_http_client()?;

    // hardcoding the IP address for now. When we implement IPAM we will be unique IP address for each VM 
    tap::setup_actor_tap(actor_uuid.clone(), "169.254.17.1".to_string()).await?;

    let fc_start_time = Instant::now();

    fcstarter::start(client.clone(), actor_uuid).await?;

    println!("firecracker started, took {:?}", fc_start_time.elapsed());


    let kernel_path = "/var/lib/ateomfc/vmlinux";
    let root_fs_path = "/var/lib/ateomfc/rootfs.ext4";
    let mut booter = booter::Booter::new(client.clone(), String::from(kernel_path), String::from(root_fs_path), docker_image_ext4_path, docker_image_rw_upper_path, actor_uuid);

    let boot_time = Instant::now();
    booter.boot().await?;
    println!("booter started, boot took {:?}, starting virtual sock now ", boot_time.elapsed());
    // now connect to the af_sock async
    virtual_sock::start_connect(booter.get_uds_path()).await?;

    loop {
        std::future::pending::<()>().await
    }
    Ok(())
}

fn get_reqwest_http_client() -> anyhow::Result<reqwest::Client> {
    let client = reqwest::Client::builder()
        .unix_socket(fcstarter::SOCKET_PATH)
        .build()?;
    Ok(client)
}
