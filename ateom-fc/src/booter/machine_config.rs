use crate::booter::Booter;
use serde::Serialize;

const MACHINE_CONFIG_API_PATH: &str = "http://localhost/machine-config";

#[derive(Serialize)]
pub struct MachineConfiguration {
    #[serde(rename = "vcpu_count")]
    pub vcpu_count: u32,
    #[serde(rename = "mem_size_mib")]
    pub memory_size_mib: u32,
    #[serde(rename = "smt")]
    pub smt: bool,
    #[serde(rename = "track_dirty_pages")]
    pub track_dirty_pages: bool,
}

impl Booter {
    pub(crate) async fn set_machine_configuration(&self) -> anyhow::Result<()> {
        let payload = MachineConfiguration {
            vcpu_count: 1,
            memory_size_mib: 128,
            smt: false,
            track_dirty_pages: true,
        };
        let response = self
            .client
            .put(MACHINE_CONFIG_API_PATH)
            .json(&payload)
            .send()
            .await?;
        Self::handle_response(response).await
    }
}
