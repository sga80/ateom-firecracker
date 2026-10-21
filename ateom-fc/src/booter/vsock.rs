use crate::booter::Booter;
use serde::Serialize;
use std::path::Path;
use uuid::Uuid;

const VSOCK_API_PATH: &str = "http://localhost/vsock";
// this CID is constant as it doesn't matter. it has to be >= 3. The uds path needs to have uniqueness in it
pub const GUEST_CID: u32 = 5;

#[derive(Serialize)]
pub struct VSockPayload {
    #[serde(rename = "guest_cid")]
    pub guest_cid: u32,
    #[serde(rename = "uds_path")]
    pub uds_path: String,
}

impl Booter {
    pub(crate) async fn vsock(&mut self) -> anyhow::Result<()> {
        let uds_path = create_uds_path().await?;
        let payload = VSockPayload {
            guest_cid: GUEST_CID,
            uds_path: uds_path.clone(),
        };
        let response = self.client.put(VSOCK_API_PATH).json(&payload).send().await?;
        Self::handle_response(response).await?;
        self.uds_path = uds_path;
        Ok(())
    }
}

async fn create_uds_path() -> anyhow::Result<(String)> {
    let actor_uuid = Uuid::new_v4();
    let uds_path = format!("/run/ateomfc/{actor_uuid}/vsock.sock");

    let path = Path::new(&uds_path);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    //let _ = tokio::fs::File::create(path).await?;
    Ok(uds_path)
}
