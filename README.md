# ateom-firecracker

[![License](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)

A from-scratch Rust reimplementation of a Firecracker-microVM sandbox
backend for [Agent Substrate](https://github.com/agent-substrate/substrate),
built as a personal proof-of-concept alongside the project's existing
`ateom-gvisor` and `ateom-microvm` backends. Not yet integrated with
Substrate — exercised standalone for now.

> **⚠️ Research project / proof-of-concept — not for production use.**
> This is a personal POC, built to explore whether a from-scratch Firecracker
> backend is viable for Agent Substrate. It has not been security-reviewed,
> is missing basic production concerns (see "Out of scope" in
> [`docs/dev/ateom-firecracker-milestones.md`](docs/dev/ateom-firecracker-milestones.md):
> jailer/seccomp hardening, CSI/volumes, observability, error-path robustness,
> etc.), and several pieces of networking setup used only for local
> development (see `scripts/`) are explicitly disposable, not hardened. Use
> at your own risk; do not run untrusted workloads through this code.

## Architecture

Three things sit nested inside one worker pod: the long-running
`ateom-firecracker` daemon, the guest-init binary it talks to, and the
actor's own container process.

![Architecture diagram: a Kubernetes node containing a worker pod, which contains the ateom-firecracker process, which forks and execs firecracker over its REST API; firecracker hosts a hardware-trapped guest boundary, inside which a guest kernel runs ateom-firecracker-init as PID 1, which execs the actor's container process. A vsock control channel runs from ateom-firecracker, through firecracker, across the guest boundary, to ateom-firecracker-init.](docs/dev/architecture.svg)

Solid grey arrows are plain process mechanics (fork/exec, the REST API). The
dashed teal line is the vsock control channel — it leaves `ateom-firecracker`,
runs through the `firecracker` process, crosses the hardware-trapped guest
boundary, and lands on `ateom-firecracker-init`, which only then execs the
actor's own process.

### What ateom-firecracker actually does

**Process lifecycle**
- Is the worker pod's own entrypoint — the first thing the pod runs.
- Forks + execs `firecracker` per active actor, and stays its parent for its whole life.
- Reaps an orphaned Firecracker child if it's ever reparented — the same duty a real PID 1 has, one level down.
- On `SIGTERM`: stops accepting new workloads, lets in-flight RPCs finish, then drains.

**Driving the VMM**
- Configures boot-source, drives, vsock and machine-config over Firecracker's REST API.
- Drives pause → snapshot → shutdown for checkpoint (M6, not yet built).
- Relaunches a fresh process and restores it from a snapshot to resume (M6, not yet built).

**Talking to the guest**
- Runs the host side of the vsock channel to `ateom-firecracker-init`: run, exec, stats, shutdown.
- Forwards the actor's stdout/stderr back to the pod's own stdout, labeled.

**Control-plane integration**
- Reports this worker's capacity (sandbox class, slot count) on startup (M5, not yet built).
- Serves `RunWorkload` / `CheckpointWorkload` / `RestoreWorkload` over gRPC (M5, not yet built).
- M4 (tap networking) shipped; M5 (`ateompb.AteomServer` integration) is next — see "Status" below.

### What's actually running, host to guest

Neither channel crosses straight from host to guest — both pass through
`firecracker`, which is the only process with a door into the guest. vsock
has two distinct hops: a one-time boot-time call that tells firecracker to
create the vsock device (ends inside firecracker — the guest never sees this
call), and a separate runtime connection that actually carries ping/exec
through to guest-init. Networking works the same way: `ateom-fc` hands
firecracker a tap device's name over `PUT /network-interfaces`, firecracker
opens the tap and presents it as `eth0`, and `guest-init` then configures
that `eth0` itself (address, link-up, default route) via raw ioctls. The
hardware trap itself sits only between firecracker and the guest kernel it
runs, not between `ateom-fc` and firecracker, which talk over an ordinary
REST/socket call.

![Diagram showing ateom-fc on the host preparing images, booting the microVM, talking to the guest over vsock, and setting up tap networking, all passing through a firecracker VMM process in the middle, with the hardware-trapped boundary sitting only between firecracker and the guest kernel it runs.](docs/dev/architecture-host-guest.svg)

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

## License

Apache License 2.0 — see [`LICENSE`](LICENSE).
