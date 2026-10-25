# ateom-firecracker

A from-scratch Rust reimplementation of a Firecracker-microVM sandbox
backend for [Agent Substrate](https://github.com/agent-substrate/substrate),
built as a personal proof-of-concept alongside the project's existing
`ateom-gvisor` and `ateom-microvm` backends. Not yet integrated with
Substrate — exercised standalone for now.

**Architecture diagram:** https://claude.ai/artifact/UJwzfLUvxm5pyoAgwtXTXR

## Layout

- `ateom-fc/` — the host daemon (async, tokio). Starts Firecracker, drives its
  REST API (boot-source, drives, vsock, machine-config, start), and talks to
  the guest over vsock.
- `guest-init/` — the guest's PID 1 (sync). Mounts the OCI image + per-actor
  overlay, runs a ttrpc server over vsock, execs the actor's process.
- `ateomfcpb/` — the shared proto crate, with dual async/sync codegen so the
  same `.proto` serves both sides.
- `scripts/` — throwaway dev tooling, not part of the real backend (e.g.
  `mac-reachability.sh`, which bridges a running actor's network namespace
  out to the Mac host for manual testing — disposable test scaffolding,
  not something that survives Substrate integration).

## Status

| Milestone | What it proved | Status |
|---|---|---|
| M1 | Boot + vsock liveness ping | done |
| M2 | Single rootfs, blocking exec | done (superseded by M3) |
| M3 | OCI image → per-actor overlay rootfs, fire-and-forget exec | done |
| M4 | Tap networking — host + guest addressing, Firecracker wiring, verified reachable from the Mac host | done |

Full milestone plan: [`docs/dev/ateom-firecracker-milestones.md`](docs/dev/ateom-firecracker-milestones.md).
Per-milestone notes and lessons learned: [`docs/dev/`](docs/dev/).

## Dev environment

Developed and tested inside a Lima VM with nested virtualization enabled
(`vmType: vz`, requires Apple Silicon + macOS 15+), with `/dev/kvm` available
inside the guest. See `docs/dev/` for milestone-specific setup notes.

## Reference notes

`docs/*.md` has working notes on the specific Firecracker REST API calls this
project drives: [`boot-source`](docs/boot-source.md), [`drives`](docs/drives.md),
[`machine-config`](docs/machine-config.md), [`vsock`](docs/vsock.md),
[`actions`](docs/actions.md), and guest-side [`mount-virtual-fs`](docs/mount-virtual-fs.md).
