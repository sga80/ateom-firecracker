use crate::booter::Booter;
use serde::Serialize;

const NETWORK_API_PATH: &str = "http://localhost/network-interfaces/eth0";

#[derive(Serialize)]
pub struct NetworkInterfaces {
    pub iface_id: String,
    pub host_dev_name: String,
    pub guest_mac: String,
    // we are not adding the 3 other attributes, mtu, and rate limiters yet

}

impl Booter {
    pub(crate) async fn create_network_interface(&self) -> anyhow::Result<()> {
        let payload = NetworkInterfaces {
            iface_id: "eth0".to_string(),
            host_dev_name: "tap0".to_string(),
            guest_mac: "02:00:00:00:00:01".to_string(),
        };
        let response = self.client.put(NETWORK_API_PATH).json(&payload).send().await?;
        Self::handle_response(response).await
    }
}
