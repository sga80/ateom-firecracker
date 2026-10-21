# `PUT /actions`

Step 5 of 5 in the Firecracker boot sequence — the one that actually
triggers boot. Everything configured in the previous four calls
(`boot-source`, `drives`, `vsock`, `machine-config`) only takes effect once
this fires.

## API shape

No ID in the URL, same as `/vsock` and `/machine-config`.

| Field | Type | Required | Notes |
|---|---|---|---|
| `action_type` | string | yes | The action to take |

```json
{
  "action_type": "InstanceStart"
}
```

Same `204 No Content` success / `400` + `{"fault_message": "..."}` failure
shape as the rest of the boot sequence.

## A failure here might not actually be about this call

Firecracker doesn't necessarily validate everything fully on each earlier
`PUT` — a problem with an earlier step (a bad kernel path, a malformed
drive) can surface here instead, on the call that looks unrelated to what's
actually wrong. If this one comes back with a `fault_message` that seems to
point at the kernel or the drive, the bug is likely in `bootSource()` or
`setupRootFSDrive()`, not in `startInstance()` itself.

## The various `action_type` values

- **`InstanceStart`** — boots the configured microVM. The one this project
  uses.
- **`SendCtrlAltDel`** — sends the Ctrl+Alt+Del key sequence to the guest,
  which (on a guest that handles it, same convention as real hardware)
  triggers a graceful shutdown/reboot request rather than yanking power.
  **Likely not usable for this project**: this relies on the legacy i8042
  keyboard-controller convention, which has no equivalent on ARM — and this
  whole stack is `linux/arm64` (Lima/`fcvm`, Docker Desktop on the M3). Worth
  confirming directly against Firecracker's current docs before relying on
  it. For graceful guest shutdown here, the path is more likely to be
  signaling over vsock instead (see `docs/vsock.md`'s "graceful shutdown"
  use case) — telling `ateom-init` to stop the actor cleanly, rather than
  `SendCtrlAltDel`.
- There was historically also a `FlushMetrics` action in older Firecracker
  versions; unconfirmed whether it's still present in the version this
  project pins (`v1.13.1`) — worth checking Firecracker's current
  `swagger.yaml`/API docs directly rather than relying on this note if it
  ever matters.

## `InstanceStart` succeeding ≠ the guest finished booting

Worth being precise about what `GET /`'s `state` field actually reflects
once this call succeeds. It flips from `"Not started"` to `"Running"`
almost immediately after `InstanceStart` succeeds — essentially at normal
API-call latency (milliseconds) — because it reflects **Firecracker's own
view of itself** ("have I started executing the vCPU threads"), not "has
the guest kernel finished booting and reached userspace." Firecracker has no
visibility into what's happening *inside* the guest's boot process — it
doesn't parse kernel logs, doesn't know when `ateom-init` starts running.

So **polling `GET /` for `"Running"` is not a reliable signal that the guest
actually booted successfully** — it only confirms Firecracker began
executing, not that the kernel didn't panic or that `ateom-init` came up.
That's exactly why the vsock "alive" message (see `docs/vsock.md`) is the
real completion signal for this milestone, not this state field.

The actual guest boot time (`InstanceStart` → receiving `ateom-init`'s
message over vsock) is a separate thing worth measuring directly once the
full loop is wired up — Firecracker's own published figure for a minimal
kernel+rootfs is around ~125ms, but that's specific to their benchmark
setup; this project's actual number (kernel config, rootfs size, and
`fcvm`'s own nested-virtualization characteristics all affect it) should be
timestamped and measured, not assumed.
