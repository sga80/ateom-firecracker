use crate::booter::Booter;
use serde::Serialize;

const ROOT_FS_DRIVE_ID: &str = "rootfs";
const IMAGE_DRIVE_ID: &str = "image";

const RW_UPPER_DRIVE_ID: &str = "rwupper";
const DRIVES_API_PATH: &str = "http://localhost/drives/rootfs";

const IMAGE_DRIVE_API_PATH: &str = "http://localhost/drives/image";
const RW_UPPER_DRIVES_API_PATH: &str = "http://localhost/drives/rwupper";

#[derive(Serialize)]
pub struct DrivePayload {
    #[serde(rename = "drive_id")]
    pub drive_id: String,
    #[serde(rename = "path_on_host")]
    pub path_on_host: String,
    #[serde(rename = "is_root_device")]
    pub is_root_device: bool,
    #[serde(rename = "is_read_only")]
    pub is_read_only: bool,
}

impl Booter {
    pub(crate) async fn setup_root_fs_drive(&self) -> anyhow::Result<()> {
        let payload = DrivePayload {
            drive_id: String::from(ROOT_FS_DRIVE_ID),
            path_on_host: self.root_fs_path.clone(),
            is_root_device: true,
            is_read_only: false,
        };
        let response = self.client.put(DRIVES_API_PATH).json(&payload).send().await?;
        Self::handle_response(response).await
    }
    pub(crate) async fn setup_image_drive(&self) -> anyhow::Result<()> {
        let payload = DrivePayload {
            drive_id: String::from(IMAGE_DRIVE_ID),
            path_on_host: self.docker_image_ext4_path.clone(),
            is_root_device: false,
            is_read_only: true,
        };
        let response = self.client.put(IMAGE_DRIVE_API_PATH).json(&payload).send().await?;
        Self::handle_response(response).await
    }
    pub(crate) async fn setup_rw_upper_docker_layer(&self) -> anyhow::Result<()> {
        let payload = DrivePayload {
            drive_id: String::from(RW_UPPER_DRIVE_ID),
            path_on_host: self.docker_image_upper_ext4_path.clone(),
            is_root_device: false,
            is_read_only: false,
        };
        let response = self.client.put(RW_UPPER_DRIVES_API_PATH).json(&payload).send().await?;
        Self::handle_response(response).await
    }
}
