# `PUT /boot-source`

Step 1 of 5 in the Firecracker boot sequence. Supplies the kernel — the
other half of what a guest needs to boot is a root filesystem, supplied
separately by the `drives` call (see `docs/drives.md`).

## What it's for

A microVM's kernel and its root filesystem are two independent concerns,
configured separately: Firecracker doesn't bundle a BIOS/bootloader that
discovers an ISO the way QEMU can — you hand it a raw kernel and a raw disk
image directly, as two separate API calls. `boot-source` is the kernel half.

## Payload

| Field | Type | Required | Notes |
|---|---|---|---|
| `kernel_image_path` | string | yes | Path to the kernel image, on the **same filesystem `ateom-fc` itself sees** — i.e. inside `ateom-fc`'s own container. Firecracker is a child process of `ateom-fc` (same `fork`+`exec`, inherited namespaces), so this is never a path "inside the guest" — the guest doesn't exist yet at the point this call is made. |
| `boot_args` | string | no | The kernel command line — see below. |
| `initrd_path` | string | no | Not used here; we boot straight off the rootfs drive, no initrd. |

```json
{
  "kernel_image_path": "/path/to/vmlinux",
  "boot_args": "console=ttyS0 reboot=k panic=1 pci=off"
}
```

`boot_args` is a **single string**, not an array — the traditional Linux
kernel command line, exactly as a bootloader like GRUB has always passed it
(its `append` line), just carried through Firecracker's API as one JSON
string value.

## Why the kernel needs a command line at all

The kernel needs some initial configuration the moment it starts, before any
filesystem is even mounted — so there's nothing like `/etc/*` for it to read
yet. Whatever loads the kernel into memory (GRUB on real hardware,
Firecracker here) also writes a single string into a known spot in memory
that the kernel reads as its very first step. The kernel command line *is*
that string — the one and only way to hand the kernel configuration before
it's properly running.

## The four boot args we use, and why each one exists

```
console=ttyS0 reboot=k panic=1 pci=off
```

- **`console=ttyS0`** — tells the kernel which device is its console: where
  boot messages, logs, and panics get written. `ttyS0` is the first serial
  port, which Firecracker gives the guest specifically so this has somewhere
  to go. Without it, the kernel has no idea where to send its own
  diagnostics — if something goes wrong early in boot, there would be
  nothing to see at all.
- **`reboot=k`** — tells the kernel *which mechanism* to use when something
  asks it to reboot. Real hardware has several possible methods (BIOS call,
  ACPI, triple-fault, etc.); `k` means "keyboard controller," an old but
  reliable method Firecracker's minimal device model actually implements.
  The default/other methods assume hardware (a BIOS, ACPI tables) that
  simply isn't present in a microVM, so picking the wrong one means a reboot
  request just hangs.
- **`panic=1`** — if the kernel hits a fatal, unrecoverable error, reboot
  automatically after 1 second instead of freezing forever displaying the
  panic message. Matters specifically because nothing is watching a serial
  console in this system — a crashed guest that just hangs forever is a
  resource nobody comes to manually reboot.
- **`pci=off`** — tells the kernel not to bother probing for a PCI bus at
  all. Firecracker deliberately doesn't implement one — its virtio devices
  use a simpler MMIO-based transport instead, part of the same minimalism
  that also means no virtio-fs (see `docs/vsock.md` for the related
  discussion of why Firecracker's device model is intentionally tiny).
  Without this flag, the kernel wastes boot time looking for a bus that will
  never answer.

Three of the four exist purely because Firecracker is *not* emulating real
PC hardware (no BIOS, no ACPI, no PCI) — the boot args have to tell the
kernel "don't expect the things a normal machine would have." The fourth
(`console`) exists so there's any visibility into boot at all.

## Response

Success is **`204 No Content`** — not `200`. This is true of every config
`PUT` in the boot sequence (`boot-source`, `drives`, `vsock`,
`machine-config`); only `GET /` (the health check) returns `200`. Checking
for `200` on a config call will silently treat every success as a failure.

Failure is a `400` with a body shaped like `{"fault_message": "<string>"}` —
worth reading directly, since it usually says exactly what's wrong (bad
path, conflicting config, etc.).
