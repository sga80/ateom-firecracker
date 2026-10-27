use anyhow::{bail, Context};
use futures_util::stream::TryStreamExt;
#[cfg(target_os = "linux")]
use nix::sched::{setns, CloneFlags};
use rtnetlink::{Handle, LinkUnspec, NetworkNamespace};
use std::fs::{File, OpenOptions};
use std::net::IpAddr;
use std::os::fd::AsRawFd;

#[cfg(target_os = "linux")]
nix::ioctl_write_ptr_bad!(tunsetiff, 0x400454ca, libc::ifreq);
#[cfg(target_os = "linux")]
nix::ioctl_write_int_bad!(tunsetpersist, 0x400454cb);

#[cfg(target_os = "linux")]
pub async fn setup_actor_tap(actor_uuid: &str, ip_address: String) -> anyhow::Result<()> {
    let tap_name = "tap0";
    let actor_uuid_clone = actor_uuid.to_string().clone();
    // if !std::path::Path::new("/var/run/netns").exists() {
    //     fs::create_dir_all("/var/run/netns")?;
    // }
    // first create the net namespace for this actor
    // the netns will give each actor its own network stack thus isolating it from other actors
    match NetworkNamespace::add(format!("{}", actor_uuid)).await {
        Ok(_) => { println!("netns created for actor {}", actor_uuid) }
        Err(e) => {
            println!("error creating network namespace, failed with error {}", e);
            bail!("cannot create netnamespace {}",e)
        }
    };
    // The operation to create the tap is to create it inside the new namespace
    // we first to set the namespace to the new namespace , however we are running this in async
    // Linux for ioctl operations requires that the same thread does the operation of opening the tun device and setting the net namespace and creating the tap0 . Therefore we spawn a blocking thread to do this.
    // for us to this currently, we first need to get the current ns , then change it to the actor namespace, create the tap and then change it back to the original namespace
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<(Handle)>
        {
            println!("opening the current namespace ");
            // get the current namespace by opening the FD to the current namespace
            let current_ns = File::open("/proc/self/ns/net")?;
            println!("getting the FD for the actor namespace");
            // get the FD to the actor namespace which was created above
            let actor_ns = File::open(format!("{}{}", rtnetlink::NETNS_PATH, actor_uuid_clone))?;
            // now change the namespace to the actor namespace
            setns(actor_ns, CloneFlags::CLONE_NEWNET)?;

            // sgavulla: I was out of depth here as I dont know how to write unsafe code so took Claude's help here. I went line by line and tried to get an understanding what is actually happening here and wrote some comments
            // now inside the actor's namespace - create the tap device
            // open the file rw mode for the tun's generic code . this is not a data file, but opening code into tun's generic control device
            // multiple threads opening this device is OK , the open creates its own FD
            println!("opening the tun control device for TUN ");
            let tun_file = OpenOptions::new().read(true).write(true).open("/dev/net/tun")?;

            //libc::ifreq - is the generic interface request struct used by kernel which reuses it on multiple different ioctls network calls
            // the below code copies the tap name to the ifr_name field of the ifr struct as expected by c code
            let mut ifr: libc::ifreq = unsafe { std::mem::zeroed() };
            for (i, &b) in tap_name.as_bytes().iter().enumerate() {
                ifr.ifr_name[i] = b as libc::c_char;
            }
            // IFF_TAP - create a layer 2 interface (the TAP interface). IFF_NO_PI disables the packet information header like distinguishing from IP4 and IPv6
            // IFF_NO_PI - this is because firecracker's backend doesnt understand the packet info header and if we send it, firecracker will parse it incorrectly
            ifr.ifr_ifru.ifru_flags = (libc::IFF_TAP | libc::IFF_NO_PI) as libc::c_short;

            // we use the ioctl to call the device driver to create the tap device.
            // this call creates the tap0 in the actor net namespace and binds tun file FD to the tap device.
            unsafe { tunsetiff(tun_file.as_raw_fd(), &ifr as *const libc::ifreq)? };
            // a tap is destroyed once every fd referencing it closes - set TUNSETPERSIST so it
            // survives tun_file being dropped here, since firecracker opens it again later by
            // name, from a different process with its own fd.
            unsafe { tunsetpersist(tun_file.as_raw_fd(), 1)? };

            // we need to assign an IP to this tap device
            // we will use rtnetlink to set this up . rtnetlink uses messages back and forth and hence we need a connection
            let (connection, handle, _message) = rtnetlink::new_connection()?;
            tokio::spawn(connection); // connection is a future and hence spawning it to poll.
            // the connection creates a socket which has the namespace tag on it so returning the handle is good here as we are still talking to the same socket that in that namespace

            // switch back to the original namespace before this thread returns to tokio's
            // blocking pool - otherwise a later, unrelated spawn_blocking call could land on
            // this same OS thread while it is still inside the actor's namespace.
            // unwrap is used here because if we fail to switch back we are in an unrecoverable state. We don't know which namespace we are in and thus cannot make any meaningful progress.
            setns(current_ns, CloneFlags::CLONE_NEWNET).unwrap();


            Ok(handle)
        }
    );
    let handle = result.await??;
    // get the tap0 index
    let mut links = handle.link().get().match_name("tap0".to_string()).execute();
    let link = links.try_next().await.context("looking up tap0 interface")?;
    if let Some(link) = link {
        let index = link.header.index;
        // assign the IP with CIDR block as 30
        // below is  equivalent to ip addr add 169.254.17.1/30 dev tap0.
        handle.address().add(index, ip_address.parse::<IpAddr>()?, 30).execute().await?;
        // now bring up the tap
        // below is equivalent to ip link set tap0 up
        handle.link().set(LinkUnspec::new_with_index(index).up().build()).execute().await?;
    } else {
        bail!("cannot get the tap0 index, tap0 not found");
    }
    // note: we spawned a connection in the blocking closure and didnt close it. This is closed when this function exists, which will drop the handle and in turn closes the connection
    Ok(())
}