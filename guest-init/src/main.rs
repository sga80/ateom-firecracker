mod virtual_fs;
mod guest_agent;
mod network;

use std::time::Instant;

#[cfg(target_os = "linux")]
fn main() -> anyhow::Result<()> {
    let start = Instant::now();
    println!("starting guest init process ");
    // 1) setup eth0 network interface
    network::setup_eth0()?;
    // 2) mount the /prod /sys and /dev drives
    let _ = virtual_fs::mount_virtual_filesystems()?;
    println!("filesystems mounted , starting ttrpc server");
    // 3) open the connection back to the host by opening the vsock
    let mut server = guest_agent::get_ttrpc_server()?;
    server.start()?;

    println!("guest init boot completed in {:?}", start.elapsed());
    

    // start() spawns its worker threads and returns immediately — `server`
    // must stay alive (dropping it tears the listener down), and PID 1 must
    // never return (the kernel panics: "Attempted to kill init!"). park()
    // blocks this thread forever at zero CPU cost; looped because park() can
    // wake spuriously per its own docs.
    loop {
        std::thread::park();
    }
}