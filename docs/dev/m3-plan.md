# M3 plan: OCI image → per-actor rootfs (Lima-only, no Substrate yet)

Not implemented yet — records the scope decided before implementation
starts, same pattern as `m2-plan.md`.

## Scope (from `ateom-firecracker-milestones.md`)

**Done when:** an arbitrary small OCI image boots and runs unmodified —
not the hardcoded `busybox echo` M2 landed with.

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
3. **Single `rootfs.ext4`, not a lower/upper split, for this pass.** The
   lower/upper split's entire justification (avoid rebuilding the shared
   base image on every actor launch) doesn't matter without real
   launch-frequency pressure — revisit specifically when Substrate
   integration (M7/M8) makes that a real concern, not before. (Separately
   confirmed as the right production pattern when that time comes — see
   "Validated by real-world practice" below.)
4. **Extract the real entrypoint via `docker inspect <image>`**
   (`Config.Entrypoint`/`Config.Cmd`) — no manual image-manifest parsing.
5. **`ExecRequest` carries the real command**, replacing the hardcoded
   `busybox echo` from M2 — this was always going to be needed regardless
   of the rootfs question.
6. **`booter::drives` stays single-drive** — no change needed there for
   this pass.

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
it. The only genuinely disposable artifact is `deploy/Dockerfile.ateom-fc`'s
rootfs-baking step (the `busybox-static`/`mkfs.ext4` block baked in at
*build* time) — once `ateom-fc` builds its own rootfs at *runtime*, that
step has nothing left to do.

### Validated by real-world practice (for when lower/upper gets built)

E2B (a real Firecracker-based sandboxing company, running "thousands of
microVMs") uses exactly the lower/upper pattern already decided for
later: one shared read-only base rootfs, plus a separate per-VM writable
overlay, specifically to avoid copying the base image per instance — same
justification already reasoned through here, now independently confirmed
by a real production deployment.

One real gotcha to handle when that's built: Firecracker's own API spec
documents `drive_id` only as a host-side config identifier — it does
**not** guarantee any specific mapping to the guest's `/dev/vdX` device
name. `firecracker-containerd`'s own drive-mounts proposal flags this
explicitly as a known problem. Likely follows attach order in practice,
but unverified — confirm empirically in `fcvm` (attach two drives, check
which becomes `/dev/vda` vs `/dev/vdb`) rather than assuming, before
`guest-init` relies on a fixed order to tell lower and upper apart.

## Not changed by this simplification

- `guest-init`'s mount logic — still mounts one plain ext4 rootfs, no
  overlay mount added.
- The actual `Exec` RPC mechanism (spawn, `wait()`, real exit code) — M2's
  version already does the right thing; M3 only changes *what* gets
  spawned (the image's real entrypoint) and *where the rootfs came from*,
  not the exec/reap mechanics themselves.
