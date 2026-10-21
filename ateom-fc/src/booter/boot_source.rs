use crate::booter::Booter;
use serde::Serialize;

// console=ttyS0 - tells the kernel to use the ttyS0 serial port for the console output of its boot. only one provided by firecracker
// reboot=k - tells the kernel to use the keyboard controller mechanism when doing a reboot. only one provided by firecracker
// panic=1 - restart after a second if there is a kernel panic
// pci=off - tells kernel to not look for pci bus as firecracker doesnt provide it
// init=/init - the kernel's default init search (/sbin/init, /etc/init, /bin/init, /bin/sh)
// never tries plain /init on a direct rootfs boot (no initramfs) — guest-init lives at
// /init, so without this the kernel finds nothing and panics ("No working init found").
const BOOT_ARGS: &str = "console=ttyS0 reboot=k panic=1 pci=off init=/init";

pub const BOOT_SOURCE_API_PATH: &str = "http://localhost/boot-source";

#[derive(Serialize)]
pub struct BootSourcePayload {
    #[serde(rename = "kernel_image_path")]
    pub kernel_path: String,
    #[serde(rename = "boot_args")]
    pub boot_args: String,
}
impl Booter {
    pub(crate) async fn boot_source(&self) -> anyhow::Result<()> {
        let payload = BootSourcePayload {
            kernel_path: self.kernel_path.clone(),
            boot_args: String::from(BOOT_ARGS),
        };
        let response = self.client
            .put(BOOT_SOURCE_API_PATH)
            .json(&payload)
            .send()
            .await?;

        Self::handle_response(response).await?;
        Ok(())
    }
}
