# Changes to Substrate itself

`ateom-firecracker` is a new sandbox backend, but Substrate's control plane
(`ateapi`, `atecontroller`, `atelet`) has no idea it exists until it's taught
to. This doc tracks every change made in the `substrate` checkout
(`~/code/go/src/substrate`, branch `firecracker`) to make that possible —
separate from this repo's own `docs/dev/ateom-firecracker-milestones.md`,
which tracks `ateom-firecracker`'s own side of the work.

**Status:** local branch only, not pushed, not a PR. Per project convention
this stays uncommitted-to-upstream until the POC is far enough along to be
worth proposing for real (see `CONTRIBUTING.md`'s process, deliberately out
of scope until then).

Each entry is marked:

- **Concrete — required.** A real gap in Substrate's support for a new
  sandbox class, independent of this being a POC. Any serious
  `SandboxClassFirecracker` integration needs this change (or something
  equivalent to it), not just this proof-of-concept.
- **POC-specific.** A shortcut that unblocks the POC but isn't the right
  long-term shape — either a narrower fix than the real design calls for, or
  a placeholder that papers over work this POC deliberately defers (see this
  repo's milestones doc, "Out of scope for the POC").

## Concrete — required

These are things any real `SandboxClassFirecracker` support would need,
POC or not.

### Register the new enum value

- `pkg/proto/ateapipb/ateapi.proto`: adds `SANDBOX_CLASS_FIRECRACKER = 3` to
  the `SandboxClass` enum, and bumps `SandboxConfig.sandbox_class`'s
  declarative-validation `+k8s:maximum` from 2 to 3 to match (the comment
  right above it says to keep these in sync).
- `pkg/api/v1alpha1/sandboxconfig_types.go`: adds the Go-side
  `SandboxClassFirecracker SandboxClass = "firecracker"` constant, and bumps
  `SandboxConfigSpec.SandboxClass`'s `+kubebuilder:validation:Enum` from
  `gvisor;microvm` to `gvisor;microvm;firecracker`.
- `pkg/api/v1alpha1/workerpool_types.go`: same enum bump, in both
  `WorkerPoolSandboxClass.Name` and `WorkerPoolSandboxClassStatus.Name`.
- Downstream generated files (`ateapi.pb.go`,
  `internal/apivalidation/zz_generated.validation.go`, the generated CRD
  YAMLs under `manifests/ate-install/generated/`) are regenerated from the
  three edits above, not hand-written — not a separate decision, just the
  mechanical consequence of them.

Without these, `SandboxConfig`/`WorkerPool` resources naming
`sandboxClass: firecracker` are rejected outright by CRD validation before
ever reaching application code.

### Wire the new enum value into the control-plane converter

- `cmd/ateapi/internal/controlapi/converter.go` (currently uncommitted):
  adds a `SANDBOX_CLASS_FIRECRACKER` case to `sandboxClassString()`'s switch.

Without this, `sandboxClassString()` silently falls through to its
`default: return ""`, and actor-template creation fails with
`sandbox_config.sandbox_class is ""` — a real missing-case bug, not a POC
simplification (every other sandbox class has a case here; firecracker was
just missing one).

### Prewarm gating applies equally to firecracker

- `cmd/atelet/sandbox_prewarm.go`: `skipConfig`'s `SandboxClassMicroVM` case
  (skip asset prewarm on nodes without `/dev/kvm`) now also matches
  `SandboxClassFirecracker`.

This is a correctness statement that's equally true for firecracker, not a
shortcut — firecracker needs `/dev/kvm` exactly like micro-VM does, so the
gating logic is right for both. (Currently a no-op in practice for
firecracker specifically, since no assets are declared for it yet — see the
POC-specific asset-check item below — but the gate itself is correct
regardless.)

## POC-specific

These unblock the POC but are shortcuts, not the eventual right shape.

### Reusing micro-VM's pod shape and capability set

- `cmd/atecontroller/internal/controllers/workerpool_apply.go`:
  - Adds `ateomFirecrackerCapabilities = []corev1.Capability{"NET_ADMIN", "SYS_ADMIN"}`,
    with the comment *"firecracker only needs the below permissions as of
    now to create tap device"* — "as of now" is a live admission that this
    is provisional.
  - Extends `maybeApplyMicroVMPodShape` to also apply to
    `SandboxClassFirecracker` (reusing micro-VM's pod shape — e.g.
    `/dev/kvm` device access — verbatim, rather than writing a
    firecracker-specific shape function).

Functionally necessary to boot Firecracker VMs at all right now, but the
capability set is explicitly a placeholder, and reusing micro-VM's shape
function works only because both backends happen to need similar host
access today. This repo's milestones doc already lists "Firecracker
jailer/seccomp hardening" under "Out of scope for the POC" — revisit this
once that hardening work actually happens, rather than assuming today's
capability set is final.

### Skipping atelet's per-architecture sandbox-assets check

**Applied and confirmed working** (atelet's logs show it bypassing the
check for firecracker and successfully calling `ateom.RunWorkload`).

- `cmd/atelet/sandbox_assets.go`'s `recordFromRequest()` unconditionally
  required `sandbox_assets.assets[runtime.GOARCH]` to be non-empty for
  *every* sandbox class, before even looking at which class it is. True and
  necessary for gVisor/micro-VM (atelet fetches their node-architecture
  binary — `runsc`, `cloud-hypervisor`/`kata-shim` — itself, at runtime, from
  object storage), but `ateom-fc` bakes its kernel and Firecracker binary
  directly into its own container image at build time
  (`deploy/Dockerfile.ateom-fc`) and asks atelet to fetch nothing. The
  precondition had no exception for that case, so every `RunWorkload` for
  firecracker failed before ever reaching `ateom-fc`, with
  `sandbox_assets has no assets for architecture "arm64"`.
- Fix applied: `recordFromRequest` now checks the sandbox class first and
  returns an empty-assets record immediately for `SandboxClassFirecracker`,
  before the arch/`GOARCH` precondition runs — mirroring the existing
  sandbox-class-specific branch right above it (`checkPauseImage`,
  gVisor-only).

This is explicitly the quick unblock, not the real design. **The concrete,
required fix** — tracked as follow-up work, not done here — is to make
`ateom-fc` match gVisor/micro-VM's actual pattern: have `SandboxConfig`
declare real per-architecture assets (the `vmlinux` kernel and the
`firecracker` binary, fetched flat the way `fetchAsset` already handles a
single binary, not extracted like gVisor's tarball), have atelet fetch and
content-address them like it does for every other backend, and have
`ateom-fc` read the fetched local paths from the request instead of
hardcoding `/var/lib/ateomfc/vmlinux`/`rootfs.ext4`. That removes the need
to rebuild and redeploy the whole `ateom-fc` image just to bump a kernel or
Firecracker version — real scope, deliberately deferred past today's
unblock.
