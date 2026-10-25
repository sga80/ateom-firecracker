// sgavulla - I was out of depth for these unsafe network setup. Claude wrote this for me 

// Brings up eth0 after Firecracker's virtio-net device creates it at boot:
// assigns the point-to-point address, sets the netmask, brings the link
// up, then adds a default route via the host-side gateway. All four are
// legacy BSD socket ioctls - same category as TUNSETIFF/TUNSETPERSIST in
// tap.rs, just a different pair of structs (ifreq for the interface calls,
// rtentry for the route). No async runtime needed, unlike the host side.

use anyhow::{bail, Context};
use std::net::Ipv4Addr;
use std::str::FromStr;

const ETH0: &str = "eth0";
const ETH0_ADDRESS: &str = "169.254.17.2";
const ETH0_NETMASK: &str = "255.255.255.252";
const GATEWAY_ADDRESS: &str = "169.254.17.1";

// Not exposed as named constants by `libc` for this target - confirmed
// against the real kernel header (include/uapi/linux/sockios.h). Same
// treatment tap.rs gives TUNSETIFF/TUNSETPERSIST.
#[cfg(target_os = "linux")]
nix::ioctl_write_ptr_bad!(siocsifaddr, 0x8916, libc::ifreq);
#[cfg(target_os = "linux")]
nix::ioctl_write_ptr_bad!(siocsifnetmask, 0x891c, libc::ifreq);
#[cfg(target_os = "linux")]
nix::ioctl_write_ptr_bad!(siocsifflags, 0x8914, libc::ifreq);
#[cfg(target_os = "linux")]
nix::ioctl_write_ptr_bad!(siocaddrt, 0x890b, libc::rtentry);

// include/uapi/linux/route.h - also not exposed by `libc` for this target.
const RTF_UP: libc::c_ushort = 0x0001;
const RTF_GATEWAY: libc::c_ushort = 0x0002;

#[cfg(target_os = "linux")]
pub fn setup_eth0() -> anyhow::Result<()> {
    // Any ioctl socket works as the handle for these calls - it isn't
    // itself a network connection, just something to issue the ioctl on.
    let sock = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0) };
    if sock < 0 {
        bail!("failed to open ioctl socket: {}", std::io::Error::last_os_error());
    }

    let result = (|| -> anyhow::Result<()> {
        set_address(sock, ETH0, ETH0_ADDRESS).context("SIOCSIFADDR")?;
        set_netmask(sock, ETH0, ETH0_NETMASK).context("SIOCSIFNETMASK")?;
        bring_up(sock, ETH0).context("SIOCSIFFLAGS")?;
        add_default_route(sock, GATEWAY_ADDRESS).context("SIOCADDRT")?;
        Ok(())
    })();

    unsafe { libc::close(sock) };
    result
}

// Builds the ifreq struct shared by SIOCSIFADDR/SIOCSIFNETMASK/SIOCSIFFLAGS
// - same shape as tap.rs's ifreq, just carrying an address instead of flags.
fn new_ifreq(name: &str) -> libc::ifreq {
    // zeroed() is required here - ifreq is a C struct with no safe Rust
    // constructor, so this is the standard way to get a valid starting
    // value before filling in fields by hand, same as tap.rs's ifreq.
    let mut ifr: libc::ifreq = unsafe { std::mem::zeroed() };
    for (i, &b) in name.as_bytes().iter().enumerate() {
        ifr.ifr_name[i] = b as libc::c_char;
    }
    ifr
}

// Packs an IPv4 address into the sockaddr the kernel expects inside
// ifreq's union. sockaddr_in and sockaddr are both 16 bytes, laid out the
// same way for this purpose - transmuting between them is the standard
// way to fill this field; it's exactly what the C header's own union does
// under the hood.
fn to_sockaddr(ip: &str) -> anyhow::Result<libc::sockaddr> {
    let addr = Ipv4Addr::from_str(ip).with_context(|| format!("invalid address {ip}"))?;
    let sin = libc::sockaddr_in {
        sin_family: libc::AF_INET as libc::sa_family_t,
        sin_port: 0,
        // from_ne_bytes, not from_be_bytes: s_addr needs its raw memory
        // bytes to literally be the octets in order (that's what network
        // byte order means as a wire format) - from_be_bytes reads them
        // as a number first, which then gets re-encoded using the
        // target's native (here: little) endianness when stored, which
        // reverses the byte order instead of preserving it.
        sin_addr: libc::in_addr { s_addr: u32::from_ne_bytes(addr.octets()) },
        sin_zero: [0; 8],
    };
    Ok(unsafe { std::mem::transmute(sin) })
}

fn set_address(sock: i32, iface: &str, ip: &str) -> anyhow::Result<()> {
    let mut ifr = new_ifreq(iface);
    ifr.ifr_ifru.ifru_addr = to_sockaddr(ip)?;
    unsafe { siocsifaddr(sock, &ifr as *const libc::ifreq)? };
    Ok(())
}

fn set_netmask(sock: i32, iface: &str, mask: &str) -> anyhow::Result<()> {
    let mut ifr = new_ifreq(iface);
    ifr.ifr_ifru.ifru_addr = to_sockaddr(mask)?;
    unsafe { siocsifnetmask(sock, &ifr as *const libc::ifreq)? };
    Ok(())
}

fn bring_up(sock: i32, iface: &str) -> anyhow::Result<()> {
    let mut ifr = new_ifreq(iface);
    ifr.ifr_ifru.ifru_flags = libc::IFF_UP as libc::c_short;
    unsafe { siocsifflags(sock, &ifr as *const libc::ifreq)? };
    Ok(())
}

fn add_default_route(sock: i32, gateway: &str) -> anyhow::Result<()> {
    // rtentry describes a route, not an interface - different struct,
    // same "zero it, then fill fields by hand" pattern as ifreq.
    let mut rt: libc::rtentry = unsafe { std::mem::zeroed() };
    // rt_dst/rt_genmask both need an explicit AF_INET sockaddr for
    // 0.0.0.0 - a fully zeroed sockaddr has sa_family = 0 (AF_UNSPEC),
    // which the kernel's route ioctl rejects with EINVAL rather than
    // treating as "any". The wildcard has to be a tagged AF_INET address
    // with zeroed bytes, not an untagged zero struct.
    rt.rt_dst = to_sockaddr("0.0.0.0")?;
    rt.rt_genmask = to_sockaddr("0.0.0.0")?;
    rt.rt_gateway = to_sockaddr(gateway)?;
    rt.rt_flags = RTF_UP | RTF_GATEWAY;
    unsafe { siocaddrt(sock, &rt as *const libc::rtentry)? };
    Ok(())
}
