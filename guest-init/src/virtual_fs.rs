use nix::mount::{mount, MsFlags};
use std::io;

#[cfg(target_os = "linux")]
pub fn mount_virtual_filesystems() -> io::Result<()> {
    mount_virtual_fs("proc", "proc", MsFlags::empty())?;
    mount_virtual_fs("sys", "sysfs", MsFlags::MS_NOSUID | MsFlags::MS_NODEV | MsFlags::MS_NOEXEC)?; // check docs/mount-virtual-fs.md for details on these flags
    Ok(())
}

#[cfg(target_os = "linux")]
fn mount_virtual_fs(name: &str, fs_type: &str, flags: MsFlags) -> io::Result<()> {
    let target = format!("/{name}");
    mount(
        Some(name),
        target.as_str(),
        Some(fs_type),
        flags,
        None::<&str>,
    )?;
    Ok(())
}
