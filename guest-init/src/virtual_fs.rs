use nix::mount::{mount, MsFlags};
use std::io;


const DOCKER_IMAGE_DIR: &str = "/mnt/image";
const DOCKER_UPPER_DIR: &str = "/mnt/upper-device";

const OVERLAYFS_WORK_DIR: &str = "/mnt/upper-device/work";
const OVERLAYFS_UPPER_DIR: &str = "/mnt/upper-device/upper";
pub const MERGED_FINAL_DIR: &str = "/mnt/upper-device/merged";

#[cfg(target_os = "linux")]
pub fn mount_virtual_filesystems() -> io::Result<()> {
    mount_virtual_fs("proc", "/proc", "proc", MsFlags::empty(), None)?;
    mount_virtual_fs("sys", "/sys", "sysfs", MsFlags::MS_NOSUID | MsFlags::MS_NODEV | MsFlags::MS_NOEXEC, None)?; // check docs/mount-virtual-fs.md for details on these flags

    // now mount the image dir by first creating the dir
    std::fs::create_dir_all(DOCKER_IMAGE_DIR)?;
    // source is /dev/vdb as /dev/vda is root and we are mounting the read only image after the root in the host
    mount_virtual_fs("/dev/vdb", DOCKER_IMAGE_DIR, "ext4", MsFlags::MS_RDONLY, None)?;

    //now mount the docker upper dir by first creating the dir
    std::fs::create_dir_all(DOCKER_UPPER_DIR)?;
    // now mount it
    mount_virtual_fs("/dev/vdc", DOCKER_UPPER_DIR, "ext4", MsFlags::empty(), None)?;
    //no create the work and upper dir
    std::fs::create_dir_all(OVERLAYFS_WORK_DIR)?;
    std::fs::create_dir_all(OVERLAYFS_UPPER_DIR)?;

    // create the merged final dir
    std::fs::create_dir_all(MERGED_FINAL_DIR)?;
    // now merge the directories using overlays
    mount_virtual_fs("overlay", MERGED_FINAL_DIR, "overlay", MsFlags::empty(), Some("lowerdir=/mnt/image,upperdir=/mnt/upper-device/upper,workdir=/mnt/upper-device/work"))?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn mount_virtual_fs(source: &str, target: &str, fs_type: &str, flags: MsFlags, data: Option<&str>) -> io::Result<()> {
    mount(
        Some(source),
        target,
        Some(fs_type),
        flags,
        data,
    )?;
    Ok(())
}
