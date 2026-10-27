mod actions;
mod boot_source;
mod drives;
mod machine_config;
pub(crate) mod vsock;
mod network_interfaces;

use anyhow::{bail, Result};
use reqwest::{Client, Response, StatusCode};
use serde::Serialize;

pub(crate) struct Booter {
    client: Client,
    kernel_path: String,
    root_fs_path: String,
    uds_path: String, // this is set when we set vsock
    docker_image_ext4_path: String,
    docker_image_upper_ext4_path: String,
    actor_uuid: String,
}

impl Booter {
    pub fn new(client: Client, kernel_path: String, root_fs_path: String, docker_image_ext4_path: String, docker_image_upper_ext4_path: String, actor_uuid: String) -> Self {
        Booter {
            client,
            kernel_path,
            root_fs_path,
            uds_path: String::new(),
            docker_image_ext4_path,
            docker_image_upper_ext4_path,
            actor_uuid,
        }
    }
    pub fn get_uds_path(&self) -> String {
        self.uds_path.clone()
    }
    pub async fn boot(&mut self) -> Result<()> {
        //1) Put boot_source the vmlinux source
        self.boot_source().await?;
        //2) Put rootfs drive system
        self.setup_root_fs_drive().await?;
        //3) Put docker read only image drive
        self.setup_image_drive().await?;
        //4) Setup RW upper docker image
        self.setup_rw_upper_docker_layer().await?;
        //5) Put guest CID by connecting the vsock
        self.vsock().await?;
        //6) Put machine config (resources)
        self.set_machine_configuration().await?;
        //7) add network interfaces
        self.create_network_interface().await?;
        //8) Put actions (InstanceStart to start the boot
        self.start_instance().await?;
        println!("all calls succeeded");
        Ok(())
    }

    async fn handle_response(response: Response) -> Result<()> {
        let status = response.status();
        println!("status code is {status}");
        if status == StatusCode::NO_CONTENT {
            return Ok(());
        }

        let response_body = response.text().await?;
        bail!("request failed with status {status}: {response_body}")
    }
}
