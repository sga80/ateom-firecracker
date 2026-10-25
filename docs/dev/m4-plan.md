# M4 plan: networking via tap (Lima-only, no Substrate yet)

**Done.** Host-side tap creation/addressing, Firecracker wiring, and
guest-side `eth0` configuration are all implemented and verified
end-to-end — the counter demo answers a request made from the Mac host,
outside Lima entirely. See "Results" below for what was actually built
and the real bugs hit along the way; the rest of this doc is the
scope/research that preceded implementation, same pattern as
`m2-plan.md`/`m3-plan.md`.

## Scope (from `ateom-firecracker-milestones.md`)

A tap device plus host-side NAT/bridge so the actor process can send and
receive network traffic, mirroring `internal/ateomnet`'s pattern for the
other backends.

**Done when:** a workload inside the microVM can serve one HTTP request
from the host — concretely, the `counter` demo's port 80 server (already
running since M3) answers a request made from the Mac host, outside Lima
entirely.

## Key decision: tap/IP setup is a real, reusable component — not a stand-in

Unlike M3's image pull (where `docker create`/`export` stood in for
Substrate's real `internal/imagecache`, a dependency this project doesn't
have yet), tap creation and IP assignment have **no Substrate
dependency** in the first place. Checked directly against real
`cmd/ateom-microvm/net.go`: `setupActorTap()` lives inside
`ateom-microvm` itself, not `atelet` — confirming tap/network mechanics
are squarely the sandbox backend's own job, the same way `booter::drives`
and `vsock` already are in `ateom-fc`. `atelet` only ever contributes an
optional policy toggle (`RunWorkloadRequest.egress_gateway`,
`ateom.proto:161` — "route this actor's egress through this gateway" or
absent = none); it never touches netlink, tap devices, or IP assignment.

So this milestone's split is:

- **Real, reusable (same shape it'll have post-Substrate-integration)**:
  - Host, in `ateom-fc`: create a tap device via netlink, assign it a
    host-side IP, bring it up, `PUT` it to Firecracker's
    `/network-interfaces` API.
  - Guest, in `guest-init`: bring up `eth0` (the virtio-net Firecracker
    presents for that tap), assign it a static IP via netlink, set a
    default route via the tap's host-side IP.
- **Test-only, Lima-specific, thrown away later**: however `ateom-fc` is
  told to do this before a real `RunWorkload` caller exists (hardcoded,
  same pattern as M3's hardcoded `Exec` command), and whatever lets a
  request from the Mac host actually reach the Lima VM and then the tap
  subnet — that routing path doesn't exist post-integration in this
  shape, since real inbound/outbound routing is `atenet`'s job (see
  below), not something the Mac-reachability test needs to resemble.

## Real precedent: `cmd/ateom-microvm/net.go`

`setupActorTap()` (cloud-hypervisor backend, not Firecracker, but the
netlink mechanics transfer directly):

- Creates the tap via `netlink.LinkAdd` — `netlink.Tuntap{Mode:
  TUNTAP_MODE_TAP, Flags: TUNTAP_NO_PI | TUNTAP_VNET_HDR}` — inside a
  **dedicated network namespace per actor** (`netns.Do(ctx, actorNetNS,
  ...)`).
- Host side of the tap gets a MAC and IP: `169.254.17.1/30`
  (`internal/ateomnet/net.go:30-47`, `hostVethLocalAddress`) — a tiny
  link-local point-to-point /30 subnet. Guest side (`eth0`) is
  `169.254.17.2/30`. Both MACs are fixed (not random) specifically so a
  Cloud Hypervisor snapshot/restore doesn't blackhole the guest's frozen
  ARP cache — not relevant to this project yet (no snapshot/restore
  milestone), but the IP/subnet convention itself is real and worth
  reusing rather than inventing a different one.
- Brings the link up (`netlink.LinkSetUp`) after setting the address.
- Returns the tap's FDs for cloud-hypervisor to adopt directly at boot —
  **doesn't transfer to Firecracker**, which instead takes a
  `host_dev_name` (the tap's name) in its `/network-interfaces` PUT
  payload, not a raw FD. This is the one real mechanical difference
  between the two backends.

**Not replicated in this milestone, deliberately**: per-actor network
namespace isolation (`netns.Handle`) and the full `cmd/atenet` router
(ingress/egress/dataplane, real production NAT/SDN) — both are scoped
out for the same reason M3 didn't build the full lower/upper production
shape until there was real pressure for it: this is a single-actor Lima
test, not a multi-tenant host.

## Why no iptables inside guest-init

Checked and ruled out: the guest doesn't need any firewall/NAT rules to
*receive* an inbound TCP connection on a listening port — a fresh kernel
with no iptables rules loaded has an all-ACCEPT default policy. All
guest-init needs is `eth0` up, a static IP, a default route, and the
counter process listening on `0.0.0.0:80`. Any NAT/DNAT needed to route a
request from outside Lima to the tap subnet belongs on the **host** side
(the Lima VM), matching the real split where `ateom-microvm` only ever
sets up the tap/addresses itself and leaves actual traffic routing to
`atenet`, never to anything running inside the guest.

## Guest needs a default route too, even though `counter` has no egress

Checked directly against real source, not assumed:
`cmd/ateom-microvm/run.go:1077-1079` sets the guest's default route
unconditionally, on every actor, via the kata agent's `UpdateRoutes` RPC:

```go
ac.UpdateRoutes(ctx, []*agentpb.Route{
    {Dest: "", Gateway: ateomnet.ActorVethGateway, Device: ateomnet.ActorVethName, Family: agentpb.IPFamily_v4},
})
```

`Dest: ""` is `0.0.0.0/0` (the default route), targeting the tap's
host-side gateway IP — set regardless of whether that actor's workload
makes any outbound calls itself. Two reasons this matters even for
`counter`:

1. **Return traffic for inbound connections still needs a route.** If
   the host's NAT path doesn't SNAT/masquerade the inbound connection so
   it appears (to the guest) to originate from the gateway IP itself,
   the guest sees the real external source address — not on its local
   `/30` — and without a default route has no path to send response
   packets back at all.
2. It costs nothing to set, and avoids a half-configured interface that
   silently breaks the moment anything else (DNS, a future egress need)
   requires it.

So `guest-init`'s route setup should include this default route (via
netlink, same as the IP/link-up calls), not skip it just because
`counter` itself never initiates outbound traffic.

## Decided: per-actor netns after all, and how tap creation actually works

Originally this milestone deliberately skipped per-actor network
namespace isolation as out of scope (single-actor Lima test, no need for
isolation yet). Reversed that: this project is being built for eventual
Substrate integration, so the netns should be built the real way now,
not retrofitted later.

Researching how to actually create a tap device in Rust surfaced
something that changes the plan:

### Tap creation is ioctl, not netlink — and that's true everywhere, not just in Rust

Checked the `rtnetlink` crate's source directly (not just its docs):
it has a real `LinkTun` builder (`handle.link().add(LinkTun::new(name).build())`),
but the per-type data it can set, `InfoTun`, is an unimplemented stub —
literally `enum InfoTun { Other(DefaultNla) }`, with no way to set
tap-vs-tun mode, `IFF_NO_PI`, or `IFF_VNET_HDR`. The file even has a
giveaway leftover bug: its error message says `"... for
IFLA_INFO_DATA(vrf)"`, copy-pasted from the VRF link type and never
fixed — a sign nobody has actually exercised this path.

This isn't a gap specific to that one Rust crate. Checked `iproute2`'s
own `ip/iptuntap.c`: even `ip tuntap add` — the real, modern, netlink-based
tool for everything else — creates tap devices via the **ioctl** path
(`open("/dev/net/tun")` then `ioctl(fd, TUNSETIFF, ...)`), not netlink. A
2009 kernel mailing list thread proposed a netlink interface for
tun/tap and it was never adopted. The kernel itself has no `RTM_NEWLINK`
path for this device type — there's nothing for any userspace library to
bind to.

Confirmed the same is true one level up, in Go: `ateom-microvm`'s
`setupActorTap` calls `netlink.LinkAdd(tap)`, which *looks* like a pure
netlink call from the call site, but the `vishvananda/netlink` library
special-cases `*netlink.Tuntap` internally (`link_linux.go`,
`linkModify()`): it detects the type, skips building any netlink message
at all, and does the literal same `open("/dev/net/tun")` +
`unix.Syscall(SYS_IOCTL, ..., TUNSETIFF, ...)` + `TUNSETOWNER` +
`TUNSETGROUP` that `iproute2` does. The library just hides the seam so
the call site reads as uniform.

**So the real split, confirmed at the kernel/iproute2/Go level, not just
guessed:**

- **Create the tap** → ioctl (`TUNSETIFF` on an fd from
  `/dev/net/tun`), called directly from `ateom-fc`'s own process — no
  subprocess, no `Command`, same category of direct syscall as the
  existing `nix::mount` calls in `virtual_fs.rs`. A tap is **not
  persistent by default**: it's torn down once every fd referencing it
  closes, so the fd must either stay open for the actor's lifetime or
  get `TUNSETPERSIST` set.
- **Assign the IP, bring the link up** → `rtnetlink` is fine here. Once
  the tap exists (regardless of how), it's a normal interface from the
  kernel's point of view — `rtnetlink`'s address-add and link-set-up
  calls don't care that the device wasn't created via netlink. This
  matches `net.go` exactly: the ioctl-based `LinkAdd` is followed by
  real netlink calls (`LinkSetHardwareAddr`, `AddrReplace`,
  `LinkSetUp`) against the now-existing interface.
- **`rtnetlink` crate choice confirmed healthy independent of the tap
  gap**: 28.3M downloads, 82 reverse dependencies, 37 releases since
  2018, actively released (most recent two versions a week apart). The
  tap limitation is a kernel-level gap everyone shares, not a sign this
  specific crate is immature or buggy like `arcbox-ext4` was.

### netns is a per-thread property — this drives the whole implementation shape

A network namespace attaches to an individual kernel thread, not a
whole process — a process can in principle have its threads scattered
across different namespaces. `setns()` only changes the calling
thread's membership. This means "enter the actor's namespace, then
create the tap" only works if both steps run on the *same* OS thread
with nothing in between that could reschedule the work onto another
one.

Confirmed this is exactly why `ateom-microvm` doesn't do a
create-in-host-then-move-into-namespace two-step (which is what early,
AI-suggested Rust snippets kept proposing, using netlink's real
`setns_by_pid`/`setns_by_fd` builder methods for the move — those do
exist and do work, but aren't what the real precedent does). Read
`internal/ateomnet/netns/netns.go` directly — its package comment states
the principle outright: *"Entering a namespace is a property of the OS
thread, so everything here locks a thread for as long as it is in one
and restores the caller's namespace before returning."* Its `Do()`
function:

1. `runtime.LockOSThread()` — pins the goroutine to its current OS
   thread for the whole call (Go's goroutines are normally free to hop
   OS threads between scheduling points, same underlying danger as
   tokio tasks moving between worker threads across `.await` points).
2. `vishnetns.Get()` — saves a handle to the *current* namespace before
   switching, specifically so it can be restored.
3. `vishnetns.Set(targetNS)` — the actual `setns()` into the actor's
   namespace.
4. Runs the caller's closure (tap creation happens here, already inside
   the target namespace — no separate move step needed).
5. `defer`-restores the original namespace, and **panics** if that
   restore itself fails, with the comment: *"Better to blow up the
   program than continue execution with one OS thread randomly in a
   different netns."*

**Rust/tokio equivalent**: `tokio::task::spawn_blocking`'s closure is
synchronous (no `.await` inside it), so it's guaranteed to run start-to-
finish on one blocking-pool OS thread — the same guarantee
`LockOSThread` gives in Go. But tokio's blocking-pool threads are
*reused* (idle keep-alive, not disposed per call), so the closure must
restore the original namespace before returning — otherwise a thread
left stuck in the actor's namespace could later be handed a completely
unrelated `spawn_blocking` call (different actor, unrelated blocking
I/O) and silently execute it in the wrong namespace. Same
save-switch-do-restore shape as Go's `Do()`, with the same severity on
restore failure (treat it as fatal, not recoverable).

### Crate choices, now settled

- **Tap creation**: direct ioctl call (`nix`/`libc`), not `rtnetlink`,
  not shelling out to `ip`.
- **Address assignment + link up**: `rtnetlink` (0.23.0, actively
  maintained — see above).
- **Namespace creation/entry**: `netns-rs` — needs its source checked
  (not just docs) to confirm its `run()` closure actually does the
  full lock-thread / save-original / restore-on-exit dance `netns.go`
  does, before relying on it for the restore-on-failure guarantee.

## Results

All of M4 is implemented and verified working:

- **Host side** (`ateom-fc/src/tap.rs`): per-actor netns created
  (`rtnetlink::NetworkNamespace::add`), tap0 created via raw ioctl
  (`TUNSETIFF`/`TUNSETPERSIST`) inside it, addressed `169.254.17.1/30`
  and brought up via a second `rtnetlink` connection opened from inside
  that same namespace (the real Substrate convention, confirmed earlier
  in this doc, used as-is — not a placeholder).
- **Firecracker wiring**: `PUT /network-interfaces/eth0` with
  `host_dev_name: "tap0"` and a fixed `guest_mac` (hardcoded, same
  reasoning as the fixed IPs — see the deferred-IPAM section below).
  Also required moving Firecracker's own process into the actor's
  namespace before it starts, via a `pre_exec` hook calling `setns()` in
  the forked child right before `exec` — not something this doc
  originally scoped, but necessary once namespace isolation was back in
  (Firecracker has to be *in* the namespace to find `tap0` by name at
  all).
- **Guest side** (`guest-init/src/network.rs`): `eth0` addressed
  `169.254.17.2/30`, brought up, default route added via
  `169.254.17.1` — all via raw BSD socket ioctls (`SIOCSIFADDR`,
  `SIOCSIFNETMASK`, `SIOCSIFFLAGS`, `SIOCADDRT`), since `guest-init` is
  synchronous and has no reason to pull in an async runtime just for
  `rtnetlink`. Real bug hit and fixed here, worth recording: the address
  bytes were packed with `u32::from_be_bytes(octets)`, which is backwards
  on this target (`aarch64-unknown-linux-musl`, little-endian) —
  `s_addr` needs its raw memory bytes to literally be the octets in
  order, not a number that gets re-encoded with the target's native
  endianness. `SIOCSIFADDR`'s loose validation let the resulting
  byte-reversed address through silently; `SIOCSIFNETMASK`'s stricter
  `bad_mask()` contiguity check (confirmed against
  `net/ipv4/devinet.c`) correctly rejected the reversed netmask with
  `EINVAL`, which is what actually surfaced the bug. Fix was
  `from_ne_bytes` instead.
- **Mac-host reachability**: confirmed working end-to-end
  (`curl 127.0.0.1:8080` from the Mac reaches the counter demo). This is
  disposable test scaffolding, not real `ateom-fc` code — real
  production inbound/outbound routing is `atenet`'s job, confirmed
  earlier in this doc. The chain is Mac → Lima's auto-forwarded port →
  `kubectl port-forward` → a `socat` relay inside the pod's own netns →
  a veth pair bridging the pod's netns to the actor's netns → DNAT in
  the actor's netns onto `169.254.17.2:80` → MASQUERADE on the way back
  out via `tap0`. Scripted (parameterized by auto-discovered pod/
  Firecracker PIDs, safe to re-run per actor) in
  `scripts/mac-reachability.sh`.

## Decisions settled during implementation (superseding "Open" below)

- Host-side subnet: `169.254.17.1/30` (tap) / `169.254.17.2/30`
  (guest) — the real Substrate convention, adopted as-is.
- Mac-host reachability: solved via disposable scaffolding (see
  "Results" above), not anything durable — this was always going to be
  thrown away, per the "Test-only, Lima-specific" split at the top of
  this doc.

## Deferred (nice-to-have, not needed for this POC): per-actor IPAM

This project only ever runs one actor at a time in Lima, so the real
Substrate `169.254.17.1/30` / `169.254.17.2/30` convention (reused
*identically* for every actor, per `net.go` above) is adopted as-is —
no per-actor address allocation. Deliberately deferred, not forgotten.

Why this would stop being safe at multiple concurrent actors: every
actor's internal address is identical (`169.254.17.2` in every
namespace). That's harmless as long as each actor's namespace stays
fully isolated, but it stops being harmless the moment a *shared*
masquerade/NAT point exists that multiple actors' traffic passes
through while still carrying that identical source address — e.g. one
`MASQUERADE` rule living in the Lima VM's default namespace, applied
to traffic from every actor. Linux's NAT/conntrack state is keyed on
`(proto, saddr, sport, daddr, dport)`, so two actors' simultaneous
`169.254.17.2:X → dst:Y` flows would be indistinguishable at that one
shared point.

The disposable Mac-reachability scaffolding (see "Results" above) *is*
exactly that kind of shared masquerade point, but it's single-actor and
thrown away, not a real multi-actor host — so it's still not yet known
whether this would bite in a real concurrent-actor deployment:
conntrack tables are
themselves per-namespace, and NAT rules can be matched by host-side
interface name rather than source address — either of which could
sidestep the collision without unique-per-actor addressing at all.
Worth checking how real `atenet` actually avoids this before assuming
IPAM is required, rather than building it preemptively.

**If/when this does need solving**: carve a pool (e.g.
`10.100.0.0/16`, ~16k `/30`s) into one unique `/30` per actor, with a
small allocator (counter + free-list, keyed by `actor_uuid`) replacing
the fixed convention above.
