use crate::booter::Booter;
use serde::Serialize;

const ACTIONS_API_PATH: &str = "http://localhost/actions";
const INSTANCE_START_ACTION_TYPE: &str = "InstanceStart";

#[derive(Serialize)]
pub struct Actions {
    #[serde(rename = "action_type")]
    pub action_type: String,
}

impl Booter {
    pub(crate) async fn start_instance(&self) -> anyhow::Result<()> {
        let payload = Actions {
            action_type: String::from(INSTANCE_START_ACTION_TYPE),
        };
        let response = self.client.put(ACTIONS_API_PATH).json(&payload).send().await?;
        Self::handle_response(response).await
    }
}
