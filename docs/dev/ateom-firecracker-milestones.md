# ateom-firecracker: POC Milestones

This document tracks the milestone plan for `ateom-firecracker`, a third
sandbox backend for Agent Substrate (alongside `cmd/ateom-gvisor` and
`cmd/ateom-microvm`), using [Firecracker](https://github.com/firecracker-microvm/firecracker)
as the VMM instead of Cloud Hypervisor.

This is a personal proof-of-concept effort, not yet an upstream contribution.
The goal is an end-to-end POC first; production concerns (security hardening,
CSI/volumes, observability, multi-arch asset builds, etc.) are deliberately
deferred until the POC proves the approach out. See "Out of scope" below.

## Key decisions

* **New `SandboxClassFirecracker`**, parallel to `gvisor`/`microvm`, rather than
  a variant under the existing `SandboxClassMicroVM`. Touches
  `pkg/api/v1alpha1/sandboxconfig_types.go`, the `ValidatingAdmissionPolicy` in
  `manifests/ate-install/sandboxconfig-validation.yaml`, and
  `cmd/atecontroller/internal/controllers/workerpool_apply.go`.
* **No Kata.** `ateom-microvm` uses Kata Containers' guest-agent model, which
  assumes virtio-fs for rootfs sharing — a device Firecracker does not
  implement by design. Rather than following Kata's Firecracker driver (which
  works around this with a devicemapper-backed block rootfs), `ateom-firecracker`
  owns a minimal custom guest-init and vsock protocol directly. Suspend/resume
  comes from Firecracker's own REST API (pause/snapshot/resume), the same
  pattern `internal/ch` already uses for Cloud Hypervisor, just VMM-specific.
* **Guest-init written in Go**, not Rust. Rust is a reasonable fit for a
  minimal guest PID 1 (smaller binary, lower baseline memory, faster runtime
  init), but Go was chosen for tooling consistency: one protobuf codegen
  pipeline shared between host and guest, free reuse of the repo's existing
  Go-only CI/lint tooling, and `CGO_ENABLED=0` already yields a static,
  libc-free binary without a second (musl) toolchain. Worth revisiting with
  real cold-boot/memory measurements if density targets later demand it.

## Milestones

### M0 — Dev environment (done)

A Lima VM (`fcvm`) with nested virtualization enabled (`vmType: vz`,
`nestedVirtualization: true`; requires Apple Silicon M3+ and macOS 15+),
`/dev/kvm` confirmed working inside the guest, and a hand-built kernel +
rootfs proven to boot under plain Firecracker.

### M1 — Bare boot + liveness ping

A throwaway Go harness launches `firecracker --api-sock`, configures
boot-source/drives/vsock via the REST API (not the CLI `--config-file` form),
boots a minimal Go guest-init as `/init` in a tiny ext4 image, and the
guest-init sends one "I'm alive" message back over vsock.

**Done when:** the harness observes that message and exits cleanly. No
workload, no OCI image, no checkpoint.

### M2 — Run a real process, relay output

Guest-init execs a hardcoded command and streams its stdout/stderr back over
vsock to the host harness.

**Done when:** `firecracker` boots, runs e.g. `echo hello from the guest`, and
the host process prints it.

**Zombie reaping, deliberately deferred (2026-10-20):** `guest-init` (PID 1)
only `wait()`s on the one command it directly spawns — it does not run a
general reap-any-child loop. In principle PID 1 is responsible for *any*
process reparented to it (if the spawned workload itself forks subprocesses
and exits before they do, those get reparented to `guest-init` by the
kernel, one image per VM or not). Decided to accept this risk for the POC
rather than build it now: one controlled workload per VM, known shape.
Revisit if the scope moves toward less-controlled/arbitrary workloads.

### M3 — OCI image → per-actor rootfs

Smaller than it first looks: `internal/imagecache` already does the
expensive, backend-agnostic work (pulling images, node-local layer caching
and dedup, composing a per-actor rootfs via a host-side overlay mount via
`imagecache.SetupBundleRootfs`) and is already shared by both
`ateom-gvisor` and `ateom-microvm` — none of that needs reinventing.

What's left, per `internal/imagecache/README.md`'s own split, is the one
part that's explicitly backend-specific: getting that already-composed host
directory in front of the workload ("runsc's gofer for gVisor, virtiofsd for
the micro-VM" — no Firecracker row exists yet). `internal/rootfs` is just
that bridge: host-composed directory → something Firecracker can attach as
a drive. guest-init execs the image's actual entrypoint instead of a
hardcoded command.

Open design question to resolve here, not before: tar the already-merged
directory into one block image per actor (simple, but loses the
shared-lower/cheap-upper-only-snapshot properties `ateom-microvm` gets from
virtio-fs), vs. doing our own lower/upper split as two block devices and
letting the guest mount the overlay itself (more work, preserves those
properties).

**Done when:** an arbitrary small OCI image (e.g. a single static Go test
binary as its own image) boots and runs unmodified.

### M4 — Networking

A tap device plus host-side NAT/bridge so the actor process can send and
receive network traffic, mirroring `internal/ateomnet`'s pattern for the other
backends.

**Done when:** a workload inside the microVM can serve one HTTP request from
the host.

### M5 — Formalize the vsock protocol

Replace the M1/M2 ad hoc messages with a real protobuf-defined protocol in
`internal/guestagent` (generated the normal way, both ends in Go): run, exec,
stats, shutdown.

**Done when:** the host harness and guest-init both talk through generated
client/server code, not hand-rolled framing.

### M6 — Suspend/resume

`internal/fc`: drive Firecracker's own REST API — pause (`PATCH /vm`),
snapshot (`PUT /snapshot/create`), shutdown, and a restore path that relaunches
`firecracker` from a snapshot directory.

**Done when:** a running stateful process (e.g. a counter) survives a full
suspend → process killed → restore → resume cycle with its in-memory state
intact.

**Clarified (2026-10-20):** "suspend" here means pause + snapshot-to-disk,
then **kill the whole Firecracker process** — not pause-and-hold it alive.
"Resume" launches a **new** Firecracker process that loads the snapshot and
continues execution transparently. Neither `guest-init` nor the workload
has any awareness this happens — no special-casing needed in the guest for
suspend/resume at all; it's entirely a host/VMM-level concern.

### M7 — Real `ateompb.AteomServer`

Turn the harness into `cmd/ateom-firecracker` proper: implement
`RunWorkload`/`CheckpointWorkload`/`RestoreWorkload`/shutdown-draining, reusing
the shared `internal/` packages (`actorlock`, `actorlog`, `ateomtunnel`,
`ateomcgroup`, `nodepath`, `serverboot`) the way `ateom-microvm` does.

**Done when:** the gRPC service responds correctly to a test client
(`grpcurl` or similar), standalone — still no Kubernetes involved.

### M8 — Wire into the control plane

Add `SandboxClassFirecracker` to `pkg/api/v1alpha1`, a VAP rule,
`atecontroller`'s pod-shape branch, `atelet` asset handling, and a
`sandboxconfig-firecracker.yaml.tmpl`.

**Done when:** a real `WorkerPool` in a real cluster schedules an actor onto
an `ateom-firecracker` worker through the normal control plane.

### M9 — E2E POC demo

Reproduce the project's own [Counter Demo](../../demos/counter/README.md)
(stateful actor, suspend/resume across many cycles) on the Firecracker
backend.

**Done when:** this is a drop-in demo of the existing `demos/counter`
scenario, just pointed at the new backend. **This is the POC finish line.**

## Out of scope for the POC

Firecracker jailer/seccomp hardening, CSI/volume support, multi-arch asset
builds, golden-snapshot warmup optimization, real metrics/observability
wiring, error-path robustness and retries, snapshot-fidelity class
distinctions (rootfs vs. memory), CRD validation hardening, and anything from
`CONTRIBUTING.md`'s contribution process (CLA, discuss-first issue, PR
sizing) — all deferred until this moves toward an actual upstream
contribution.
