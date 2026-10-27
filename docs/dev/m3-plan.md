# M3 plan: OCI image → per-actor rootfs (Lima-only, no Substrate yet)

**Status: complete.** The real `counter` demo (Substrate's own stateful
test actor) boots inside a real Firecracker microVM from a real,
overlay-merged rootfs and runs as a genuine long-lived server — confirmed
live via its own `readyz`/count log output, including real writes landing
on the per-actor writable layer. See "Final result" near the bottom for
the actual log and boot timings. Several things below turned out
differently from the original plan (noted inline) — this doc is kept as
the record of what was decided, tried, and learned, not rewritten to
pretend the final shape was the first guess.

## Scope (from `ateom-firecracker-milestones.md`)

**Done when:** an arbitrary small OCI image boots and runs unmodified —
not the hardcoded `busybox echo` M2 landed with. **Met** — with the
caveat that "arbitrary" became "the counter image specifically, with its
entrypoint hardcoded host-side" rather than discovered automatically (see
"What actually shipped vs. the original plan" below).

## Key decision: simplified for Lima, deferred from the production shape

The milestones doc's own M3 section frames this as reusing Substrate's
`internal/imagecache` (real registry pull, node-local layer caching) plus
a new `internal/rootfs` bridge, with an open question about a lower/upper
block-device split to keep per-actor launches cheap at scale.

**None of that applies yet** — this project doesn't integrate with
Substrate until a later milestone (real Substrate gets deployed in `fcvm`
and tested end to end then). Building the full production shape now, with
no real caching infrastructure or launch-frequency pressure behind it,
would be solving a problem that doesn't exist yet in this environment.

Simplified scope for this pass, structured as **two deliberately separate
stages** — this split matters, see below:

1. **Stage 1 (throwaway, Lima-only): produce a folder.** `docker create
   <image>` then `docker export <container> | tar -x -C <staging-dir>`
   gives the image's fully-merged filesystem directly (Docker's own
   overlay driver already did the layer merging) into a plain directory —
   no registry auth or layer-cache logic to reimplement. This stage's only
   job is "make a folder exist"; nothing downstream needs to know how.
2. **Stage 2 (real, reusable `ateom-fc` Rust code): fold → block device.**
   Same `mkfs.ext4 -d <staging-dir> -F rootfs.img <size>` mechanism already
   proven for the M1/M2 rootfs. Takes only a folder path as input — has no
   awareness of where that folder came from.
3. ~~**Single `rootfs.ext4`, not a lower/upper split, for this pass.**~~
   **Superseded — the lower/upper split was built in this pass after all.**
   The reasoning for deferring it (no real launch-frequency pressure yet)
   turned out to rest on an unverified assumption: whether `ateom-fc` is
   one-pod-per-actor or a long-running daemon. Checked directly against
   real `cmd/ateom-microvm` source: it's a long-running daemon handling up
   to `--max-actors` (default 1000) over its lifetime, keyed by
   `actorUid`, exactly like `ateom-gvisor`. That makes a shared,
   image-keyed lower pay off, so the split was implemented now rather than
   deferred to M7/M8. See "What actually shipped" below for
   the real drive layout and mount sequence.
4. ~~**Extract the real entrypoint via `docker inspect <image>`**~~
   **Deferred, not implemented.** `docker inspect` extraction was
   researched (confirmed real Substrate itself does the equivalent via
   `go-containerregistry` + `resolveProcessArgs`, see "Where real
   Substrate resolves this" below) but never wired into `ateom-fc`. The
   command is hardcoded host-side instead (`ExecRequest.message =
   "/counter"`), matching the explicit scope decision to stub what real
   `atelet` will eventually hand over via `RunWorkload`.
5. **`ExecRequest` carries the real command** — done, but reshaped: it
   also stopped being a "run to completion, report exit code" RPC
   entirely. See "Exec had to become fire-and-forget" below.
6. ~~**`booter::drives` stays single-drive**~~ **Superseded — three
   drives now** (root, image, rw-upper). See "What actually shipped"
   below.

### Why the folder-in-the-middle split matters

Substrate's real `internal/imagecache.SetupBundleRootfs` produces exactly
the same thing stage 1 does here: a plain merged folder, nothing more —
not a docker container, not a block device. If stage 2's Rust code is
written against "a folder path" as its only input (not "pipe `docker
export`'s output directly into `mkfs.ext4`" as one tangled step), swapping
stage 1 later — Lima's `docker export` → Substrate's real pull pipeline —
touches zero lines of the rootfs-building code. The seam is deliberate,
not incidental.

### Correction: what's actually reusable vs. throwaway

Earlier framing in this doc's history was wrong to call the `docker`-based
pull "not reusable." Since `ateom-fc` is Rust and stays Rust, *any* Rust
code written inside it — including stage 1's `docker create`/`export`/
`inspect` calls, if implemented there — is real, kept code, not throwaway,
regardless of whether Substrate integration later changes *what* triggers
it. The only disposable artifact is `deploy/Dockerfile.ateom-fc`'s
rootfs-baking step (the `busybox-static`/`mkfs.ext4` block baked in at
*build* time) — once `ateom-fc` builds its own rootfs at *runtime*, that
step has nothing left to do.

### What actually shipped: three drives, per-actor UUID, real overlay mount

**Drive layout** (`ateom-fc/src/booter/drives.rs`, attach order = device
order, confirmed empirically rather than assumed — see the gotcha noted
below):
- `vda` — root (`rootfs`), the M1 busybox boot image, `is_root_device:
  true`.
- `vdb` — `image`, the lower: `docker_lower_ext4_block_device()` builds
  it once from the flattened counter folder via `arcbox-ext4`
  (`unpack_tar`), `is_read_only: true`.
- `vdc` — `rw-upper`, the upper: `docker_upper_ext4_block_device(actor_uuid)`
  formats an *empty* ext4 image (`Formatter::new(...).close()`, no
  `unpack_tar` call — nothing to pre-populate), `is_read_only: false`.

**Per-actor identity**: `actor_uuid` (a `Uuid::new_v4()`) is generated
once in `main.rs` and threaded through to the upper-drive path
(`/run/ateomfc/<actor_uuid>/upper.ext4`) and `vsock`'s own UDS path
(`/run/ateomfc/<actor_uuid>/vsock.sock`) — resolving the earlier gap where
the UUID was local to `vsock.rs` and unavailable anywhere else.

**Guest-side mount sequence** (`guest-init/src/virtual_fs.rs`):
1. Mount `vdb` read-only at `/mnt/image` (the lower).
2. Mount `vdc` read-write at `/mnt/upper-device` (the upper device).
3. Create `upper`, `work`, and `merged` as three sibling directories
   *inside* `/mnt/upper-device` — required because overlayfs's `workdir`
   must be on the same filesystem as `upperdir` (atomic `rename()` for
   copy-up/whiteout handling doesn't work across filesystems), and
   because the merged mount target also needs to pre-exist.
4. Overlay-mount: `mount("overlay", "/mnt/upper-device/merged", "overlay",
   0, "lowerdir=/mnt/image,upperdir=/mnt/upper-device/upper,workdir=/mnt/upper-device/work")`.
   `/mnt/upper-device/merged` is what the counter binary actually runs
   against.

Naming note: the merged directory is deliberately *not* called `rootfs`
(Substrate's own `atelet` convention for this exact directory, see
`cmd/atelet/oci.go:98`) — this project already uses `rootfs` to mean the
boot root drive (`ROOT_FS_DRIVE_ID`/`root_fs_path`, `vda`), a different
thing. Reused Docker's `overlay2` naming (`merged`) instead to avoid the
collision.

### Exec had to become fire-and-forget

M2's `Exec` spawned a short-lived command and blocked on `child.wait()`
to report a real exit code — correct for `busybox echo`, but the counter
binary is a long-running web server. Blocking on `.wait()` for it means
the RPC never returns at all.

Checked how real production handles this (`cmd/ateom-microvm`):
`RunWorkload` (`run.go:234-294`) boots the actor and returns
`&ateompb.RunWorkloadResponse{}` immediately after `coldBootActorRetrying`
succeeds — it never waits for the workload process itself to exit.
Reaping is a fully separate, decoupled subsystem
(`cmd/ateom-microvm/internal/reaper/reaper_linux.go` +
`internal/childreap/childreap.go`): one background goroutine registered
for `SIGCHLD`, looping `wait4(-1, WNOHANG)` until nothing's left, for the
life of the daemon — triggered by the kernel, not tied to any RPC.

Changed `guest_agent.rs`'s `exec()` to match that shape: `spawn()` only,
return success/failure of the *spawn* (`ExecResponse.success`, replacing
the old `exit_code`), never call `.wait()`. The child keeps running after
the RPC returns.

This also **reopens the "general zombie reaping" deferral from M2** — M2
deferred it on the assumption every exec'd command was short-lived and
could be `.wait()`'d inline when convenient. That assumption is now false
for any long-running workload, so it's not just a robustness nice-to-have
anymore; it's a real gap (the spawned child has no handle in Rust to be
`.wait()`'d by anything right now, since `Child` is dropped at the end of
`exec()`). A guest-side equivalent of the production SIGCHLD reaper — or
at minimum a background thread doing `nix::sys::wait::waitpid(Pid::from_raw(-1),
WaitPidFlag::WNOHANG)` in a loop — is the next real gap to close, not
done in this pass.

### Where real Substrate resolves this (for when M3's stand-ins get replaced)

Traced the real path end to end, since this project's stand-ins
(`docker inspect`-shaped extraction, hardcoded `Exec` command) are meant
to be replaced by it later, not reimplemented forever:

- `internal/imagecache.Store.EnsureImage` pulls the image via
  `go-containerregistry` (registry API, no docker daemon) and caches its
  parsed `v1.Config` (`Entrypoint`/`Cmd`/`Env`/...), keyed by digest.
- `cmd/atelet/oci.go`'s `resolveProcessArgs`/`resolveActorEnv` combine
  that `v1.Config` with the actor template's own `command`/`args`/`env`
  (Kubernetes Pod semantics: `command` replaces Entrypoint+Cmd entirely,
  `args` alone replaces just Cmd) into a resolved
  `ateompb.ContainerSpec{Args, Env, Capabilities, Resources}`.
- `atelet` sends that already-resolved spec to whichever `ateom-*` pod
  is running, via `RunWorkload` gRPC (`main.go:563`,
  `dialAteom`/`DialAteomPod`). `ateom-gvisor` and `ateom-microvm` both
  just receive `ContainerSpec.Args`/`.Env` — neither touches image config,
  Docker, or a registry itself.

So once real `RunWorkload` integration lands, `ateom-fc`'s `ExecRequest`
(or whatever replaces it) should expect to *receive* `args`/`env` already
resolved, the same way gvisor/microvm do today — not resolve them itself.
This is why the `docker inspect`-based extraction was researched but
never built into `ateom-fc`: it would be throwaway work replicating
exactly what `atelet` already does, for a dependency this project doesn't
have yet.

### Deferred: Linux capabilities for the exec'd process

Same category as the UID/GID deferral below, and deferred for the same
reason: real Substrate's own `ateletpb.SecurityContext` *does* carry a
`capabilities` field (default set `AUDIT_WRITE`/`KILL`/`NET_BIND_SERVICE`,
resolved via `resolveCapabilities` in `oci.go:49-75`) — so, unlike the
UID/GID case, there is a real value to eventually adopt here, just not
wired up in this pass. Right now the counter process inherits
guest-init's full, unrestricted capability set. The real fix, when
picked up, is lowering the process's capability *bounding set* via
`capset()` (e.g. the `caps` crate) before `exec()`, since a capability
dropped from the bounding set can never be regained even across `exec()`
of a privileged binary.

### Validated by real-world practice

E2B (a real Firecracker-based sandboxing company, running "thousands of
microVMs") uses exactly the lower/upper pattern built in this pass: one
shared read-only base rootfs, plus a separate per-VM writable overlay,
specifically to avoid copying the base image per instance — same
justification already reasoned through here, independently confirmed by
a real production deployment.

One real gotcha, now **verified empirically rather than assumed**:
Firecracker's own API spec documents `drive_id` only as a host-side
config identifier — it does **not** guarantee any specific mapping to the
guest's `/dev/vdX` device name (`firecracker-containerd`'s own
drive-mounts proposal flags this explicitly as a known problem). Attach
order in practice did match device order: `rootfs` (attached first) →
`vda`, `image` (second) → `vdb`, `rw-upper` (third) → `vdc`, confirmed by
real boot logs in `fcvm`, not just inferred from Firecracker's docs or
third-party reports.

### Known issue: `arcbox-ext4` 0.1.2 sizing bug

Verified directly against the vendored crate source
(`~/.cargo/registry/src/.../arcbox-ext4-0.1.2/src/formatter.rs`), not
inferred from the symptom alone. Requesting a `min_disk_size` smaller than
one full ext4 block group (`block_size * 8` blocks — 128MiB for the only
supported 4096-byte block size) produces an image whose superblock
declares a full block group's worth of blocks, while the backing file on
disk is only ever truncated (`set_len`) to the originally requested,
smaller size. Mounting it then fails with a real kernel error:

```
EXT4-fs (vdb): bad geometry: block count 32768 exceeds size of device (16384 blocks)
```

Root cause (`formatter.rs:824-832`, in `close()`): when there's only one
block group, `self.size` gets bumped up to the full-group size to compute
the superblock fields, but that branch never calls `self.file.set_len()`
— and the two places that do call it later are both gated on comparisons
against the already-bumped `self.size`, so neither one fires.

Workaround in use: request `min_disk_size >= 128 * 1024 * 1024` (one full
block group) in `docker_ext4.rs`'s `Formatter::new(...)` call, regardless
of how small the actual content is.

This is a real bug in a core, not peripheral, code path of an early-stage
(0.1.2) crate — a signal to revisit this dependency (patch/upstream a fix,
or re-evaluate shelling out to real `mkfs.ext4 -d` instead) before any
production use, not just carry the workaround forward indefinitely.

### Deferred: non-root UID/GID for the exec'd process

The hardcoded `Exec` command (the counter binary) currently runs as root
in the guest — `guest-init` is PID 1 (root by necessity), and
`std::process::Command::spawn()` has the child inherit the parent's
credentials unless `.uid()`/`.gid()` are set explicitly.

Checked whether there's a real value to adopt instead of inventing one:

- The counter image itself declares no `Config.User`
  (`docker image inspect <id> --format '{{.Config.User}}'` returns empty)
  — per OCI semantics that means root (0:0) is correct, unmodified
  behavior for this specific image. There's nothing non-root to extract
  from it.
- Real Substrate has no UID/GID field in this part of its wire format
  either, checked directly: `ateletpb.SecurityContext`
  (`internal/proto/ateletpb/atelet.proto:445-447`) carries only
  `capabilities`, and `ateompb.ContainerSpec` carries only
  `args`/`env`/`capabilities`/`resources` — no uid/gid anywhere.
  `atelet` never extracts `Config.User` from a pulled image at all. This
  is a real, verified gap in production Substrate today, not a shortcut
  specific to this project.

Since both the image and upstream Substrate are silent on this, a fix
here would mean inventing a policy default (e.g. a fixed non-root UID
like `nobody`/`65534`) not derived from anything upstream — deferred for
now, alongside capability-dropping ([[ateom-firecracker-project]]), as
the same class of defense-in-depth concern rather than a correctness
blocker for running the counter binary.

## Not changed by this simplification

- `fcstarter.rs`'s Firecracker-process health-check loop
  (`HEALTH_CHECK_INTERVAL = 1s`) — the ~1s one-time cost to start the
  Firecracker process itself (see timings below) comes from this fixed
  poll interval racing the API socket's creation, not from Firecracker
  being slow. Left as-is; a real fix (shrinking the interval) was
  identified but not applied since it's a one-time cost per VM, not a
  per-request one.

## Lessons that don't fit cleanly elsewhere

- **`arcbox-ext4`'s sizing bug** (see "Known issue" above) was found by
  reading the real kernel error (`bad geometry: block count 32768
  exceeds size of device (16384 blocks)`) and matching its exact numbers
  against the vendored crate source — not by guessing from the symptom.
  The same discipline applied to the drive-order gotcha above: verify
  against real behavior/source before building on a claim, especially
  once a claim stops being "probably fine for Lima" and starts being
  something `guest-init` structurally depends on.
- **A host-side infinite loop needs the same zero-cost-blocking care as
  the guest's.** `guest-init`'s PID-1 loop uses `std::thread::park()`
  deliberately for zero CPU cost. The equivalent host-side addition to
  keep `ateom-fc` alive after boot initially used
  `loop { tokio::task::yield_now().await; }`, which doesn't block —  it
  busy-spins a full CPU core forever. Fixed to use the async equivalent
  of `park()` (a never-resolving future) instead. Same bug class, same
  fix shape, worth checking for on both sides of any future "block
  forever" code.

## Final result

The real counter server, running inside the microVM against the
overlay-merged rootfs, confirmed via live serial-console output:

```
{"time":"...","level":"INFO","msg":"Starting counter server on port 80"}
{"time":"...","level":"INFO","msg":"Wrote content to random file","fshash":"uztP6T/..."}
{"time":"...","level":"INFO","msg":"Readyz now reports OK"}
{"time":"...","level":"INFO","msg":"Count","count":0,"fshash":"uztP6T/..."}
{"time":"...","level":"INFO","msg":"Count","count":1,"fshash":"uztP6T/..."}
{"time":"...","level":"INFO","msg":"Count","count":2,"fshash":"uztP6T/..."}
```

The "Wrote content to random file" + matching `fshash` on every `Count`
line confirms real write I/O against the per-actor upper layer, not just
a process that happened to start.

Measured timings (instrumented with `std::time::Instant` on both sides):
- Firecracker process start (spawn + health-check poll): ~1.0s, one-time
  per VM, dominated by `fcstarter.rs`'s fixed 1s poll interval (see "Not
  changed" above) rather than Firecracker itself being slow.
- VM boot (all Firecracker API calls through `InstanceStart`): ~23.8ms —
  the part that recurs on every actor launch in the long-running-daemon
  model.
- Guest-init boot (mount all filesystems + overlay mount + start ttrpc
  server): ~98–165ms.
