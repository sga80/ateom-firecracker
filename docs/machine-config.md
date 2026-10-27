# `PUT /machine-config`

Step 4 of 5 in the Firecracker boot sequence. Declares the guest's compute
resources: how many vCPUs, how much memory.

## API shape

No ID in the URL (like `/vsock`, unlike `/drives/{id}`) — only one machine
config per VM.

| Field | Type | Required | Notes |
|---|---|---|---|
| `vcpu_count` | integer | yes | Number of vCPUs — `1` for M1 |
| `mem_size_mib` | integer | yes | Guest memory in MiB — `128`, matching the original hands-on walkthrough |
| `smt` | boolean | no | Simultaneous multithreading topology — see below |
| `track_dirty_pages` | boolean | no | Needed for incremental/live snapshots later — not M1's concern |

```json
{
  "vcpu_count": 1,
  "mem_size_mib": 128
}
```

## What `vcpu_count` actually controls

Ties directly to the KVM mechanics covered elsewhere: a "vCPU" isn't a piece
of hardware, it's **a thread inside the Firecracker process**, spending
almost all its time blocked inside one `KVM_RUN` ioctl call, during which
the real physical core executes the guest's instructions directly (via the
hardware-trapped VT-x/EL2 boundary).

`vcpu_count` is how many of these threads Firecracker spins up — exactly how
many CPU cores the **guest kernel** believes it has. The guest discovers
this **once, at boot**, from the virtual hardware Firecracker presents to
it, the same way a real kernel discovers CPU topology from ACPI tables on
physical hardware. It can't ask for more at runtime — there's no second core
sitting there to attempt, query, or get denied. A workload inside the guest
that spawns more threads than there are vCPUs just gets them time-sliced
across what exists, ordinary multitasking, nothing special.

### Can it exceed the real host's CPU count?

Yes. Since a vCPU is just an ordinary thread, it's scheduled by the **host's**
normal scheduler across however many real cores exist — it isn't a
reservation of a dedicated physical core. Configuring more vCPUs than the
host has real cores ("oversubscription") is completely routine in
virtualization generally. Nothing breaks; performance degrades under load
(a vCPU thread preempted off a real core effectively freezes that virtual
core's execution until it gets scheduled again) — the same symptom as
running too many ordinary processes on too few real cores.

Firecracker does cap `vcpu_count` at an upper bound (recalled as 32, worth
checking current docs if this becomes load-bearing).

### How this differs from Kubernetes CPU requests/limits

A different kind of constraint, not just a stricter version of the same
thing:

- **Kubernetes CPU requests/limits** are an **accounting** constraint,
  implemented via cgroups (CFS shares for requests, CFS quota/period for
  limits). They don't restrict *which* cores a container's threads can run
  on, or how many it can spread across — only how much aggregate CPU *time*
  it accumulates. A container with a generous limit can have threads
  scheduled across every core on the box simultaneously.
- **`vcpu_count`** is a **topology** constraint — the guest kernel never sees
  more cores than this to begin with. There's no "more capacity available
  under low contention" the way a cgroup quota could theoretically allow;
  the guest's scheduler has no concept of additional cores existing at all.

The two axes **stack**: the pod's own cgroup CPU request/limit still applies
to the whole `ateom-fc` + `firecracker` process tree exactly like any other
pod (however many vCPU threads exist, they're still just threads in that
cgroup) — and `vcpu_count` is a harder, separate ceiling one level further
in, on what the guest kernel itself can perceive.

## What `smt` controls

SMT (Simultaneous Multi-Threading — Intel's "Hyper-Threading") is a CPU
feature where one physical core presents as two (or more) logical cores,
sharing the underlying execution hardware. The `smt` field doesn't turn
hyperthreading on or off on the real hardware (Firecracker has no power over
that) — it controls whether the **vCPU topology presented to the guest**
reflects SMT sibling relationships. This matters for:

1. **Scheduling awareness** — a guest that knows which vCPUs are SMT
   siblings can avoid placing two unrelated CPU-heavy workloads on siblings
   that are actually contending for the same execution hardware.
2. **Security** — SMT siblings share real execution resources (cache,
   execution ports), and there's a documented class of side-channel
   vulnerabilities (the MDS/L1TF family) where one hardware thread can
   potentially infer what its sibling is doing. Some security-conscious
   multi-tenant environments disable SMT entirely at the host level for
   exactly this reason.

Irrelevant with `vcpu_count: 1` — SMT topology only means anything for
*pairs* of vCPUs that could be siblings; a single vCPU has nothing to pair
with.

## What `mem_size_mib` actually guarantees

Guest memory is just an ordinary anonymous `mmap`'d region inside the
Firecracker **process's own** address space (handed to KVM via
`KVM_SET_USER_MEMORY_REGION`), so it follows completely ordinary Linux
virtual memory semantics: **demand paging**. Reserving `128MiB` of address
space doesn't immediately back any of it with real physical RAM — a page is
only allocated the first time something actually touches it.

So `mem_size_mib` is a **ceiling**, not a reservation: if `ateom-init` only
ever touches ~20MiB worth of pages across its run, the host only ever backs
~20MiB of real RAM for it, even though the configured value says 128. Real
RAM usage grows lazily, tracking what's been touched, up to that fixed
maximum.

Connects to something in the real codebase: `cmd/ateom-microvm`'s
`vmmMemReserveMiB` accounts for the VMM/virtiofsd's *own* process memory — a
separate concern from the guest's memory, held back from the pod's overall
limit so the pod doesn't OOM when the guest is sized right up against it.
Kubernetes' own memory accounting (and its OOM-killer) is based on actual
resident memory (RSS), not the nominal ceiling — so a guest that's barely
touched memory keeps the pod's real usage low too, consistent with lazy
paging.

(Open question, not yet verified: whether Firecracker exposes an explicit
"eager pre-allocate everything upfront" toggle, for workloads that want to
trade guaranteed full RAM consumption for avoiding page-fault latency at
runtime. Some VMMs have this; unconfirmed whether Firecracker's basic
`machine-config` does.)

## Response

Same as every config call: `204 No Content` on success, `400` with
`{"fault_message": "<string>"}` on failure.
