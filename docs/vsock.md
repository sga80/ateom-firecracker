# `PUT /vsock`

Step 3 of 5 in the Firecracker boot sequence. Declares the device that
`ateom-fc` and `ateom-init` will use to talk to each other once the guest is
running.

## What vsock is

A socket type — same `connect`/`read`/`write` API as any socket — built
specifically for **host ↔ guest VM communication**. Why not reach for
something more familiar:

- **TCP/IP sockets** need a full virtual network stack inside the guest (a
  virtual NIC, an IP address, routing) just to send a few bytes — a lot of
  machinery for a control message.
- **Unix domain sockets (UDS)** are addressed by a filesystem path — but the
  host and guest have completely separate filesystems and separate kernels,
  so there's no shared path for them to meet at.
- **vsock** sidesteps both: addressed by `(CID, port)` instead of an IP or a
  path, and the hypervisor (Firecracker, via KVM) just wires the two ends
  directly together — no virtual NIC, no IP stack, no routing — while still
  giving you the familiar socket API on both sides.

("UDS" = Unix Domain Socket. `uds_path`, a field below, is Firecracker's
name for the path to one.)

## Why `ateom-fc` and `ateom-init` need to talk at all

Because there's a hardware-enforced wall between them (the VT-x/EPT guest
boundary) — no shared memory, no function calls across it, nothing implicit.
The only way across is a device both sides explicitly cooperate through, and
vsock is the lightweight one built for exactly this.

Two concrete reasons, one immediate and one further out:

1. **Right now (M1)**: the REST API boot sequence only confirms *Firecracker*
   accepted the configuration and started executing — it says nothing about
   whether the **guest kernel actually booted**, or whether `ateom-init`
   came up successfully rather than crashing or panicking. The only way to
   confirm the whole stack worked end to end is a signal from *inside* the
   guest — the "alive" message this milestone is building toward.
2. **Looking further ahead**, every real `AteomServer` RPC `ateom-fc` will
   eventually implement needs some guest-side cooperation to actually
   fulfill:
   - **Running the workload** — "exec this command" / the real entrypoint
     from the actor's OCI image `config.json`. The actual payload behind a
     future `RunWorkload`.
   - **Streaming the actor's stdout/stderr back** — no other path for those
     bytes to leave the guest. `ateom-init` relays them over vsock;
     `ateom-fc` forwards them to the pod's own stdout, same role real
     `ateom-microvm`'s `actorLogger` plays.
   - **Quiesce-before-checkpoint** — before `ateom-fc` pauses+snapshots the
     VM via Firecracker's own API, telling `ateom-init` to finish in-flight
     work and flush buffers first. Same role Kata's guest agent plays for
     `ateom-microvm`'s checkpoint flow, our own minimal version of it.
   - **Graceful shutdown** — telling the actor to exit cleanly on pod
     termination, rather than the VM just losing power.
   - **Resource usage / stats queries** — feeding the same kind of
     usage-emission pattern `ateomstats.UsageEmitter` does in real Substrate.
   - **Exec, for debugging** — an operator-facing "run a command inside this
     running actor," proxied through `ateom-init` since Kubernetes can't see
     into the guest at all; the real "container" from its point of view is
     just `ateom-fc`/`firecracker`.

## Clearing up "host" — it's not Firecracker

"Host" in vsock's `CID=2` sense is narrower than "the host machine." It
means: whatever is outside the hardware-virtualization boundary — which, in
this stack, includes **both** `ateom-fc` and `firecracker`, since neither
runs inside the guest. But they play different roles:

- **`firecracker`** implements the vsock *device* — plumbing. It proxies
  bytes between a Unix socket on its own side and the guest's vsock
  connection, and has nothing of its own to say to the guest.
- **`ateom-fc`** is the party that actually wants to talk to `ateom-init`. It
  goes *through* Firecracker's proxy to get there, since Firecracker is what
  implements the device at all.

So "`CID 2` = the host" really means "whatever's listening on the Unix
socket Firecracker proxies to" — in our setup, that's `ateom-fc`, with
`firecracker` as the switchboard in between, not a participant in the
conversation itself.

## API shape

`PUT /vsock` — **no ID in the URL**, unlike `/drives/{id}`, since a microVM
can only ever have one vsock device; there's nothing to disambiguate between
multiple instances of it.

| Field | Meaning |
|---|---|
| `guest_cid` | The CID assigned to the guest — must be ≥3 (0–2 reserved; 2 is always the host) |
| `uds_path` | Unix socket path **on the host side** (inside `ateom-fc`'s own container — same filesystem as `kernel_image_path`/`path_on_host`, nothing inside the guest) that Firecracker uses to proxy vsock traffic |

```json
{
  "guest_cid": 3,
  "uds_path": "/tmp/firecracker-vsock.sock"
}
```

## Does `guest_cid` need to be unique across multiple guests?

Not in Firecracker's default mode. Its vsock device is implemented entirely
in **userspace** — it doesn't register anything with the host kernel's
shared vhost-vsock subsystem. Every guest's vsock traffic is routed through
its own separate `uds_path`, so two completely different Firecracker
processes, each independently configured with (say) `guest_cid: 3`, don't
collide — the CID is just an address *within that one guest's own vsock
device*, not a host-wide registry entry.

What actually needs to be unique per actor/guest is **`uds_path`** — derived
deterministically from the actor's own identity (e.g.
`/run/ateomfc/<actor-id>/vsock.sock`), the same pattern real Substrate uses
elsewhere (`nodepath.AteomSocketPath(podUID)`). No CID-allocator or
collision-avoidance table needed.

Caveat worth being honest about: this is specific to Firecracker's default
UDS-backed vsock. There's a separate `vhost-vsock` passthrough mode (using
the host kernel's native vsock device directly, for performance) where CIDs
*would* need to be genuinely host-wide unique, since that mode shares a real
kernel resource across processes. Not relevant to M1's single-actor scope,
and worth verifying empirically before relying on it if multi-actor support
is ever built for real.

## Response

Same as every config call: `204 No Content` on success, `400` with
`{"fault_message": "<string>"}` on failure.
