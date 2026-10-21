use crate::booter::Booter;
use serde::Serialize;

const ROOT_FS_DRIVE_ID: &str = "rootfs";
const DRIVES_API_PATH: &str = "http://localhost/drives/rootfs";

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
}
