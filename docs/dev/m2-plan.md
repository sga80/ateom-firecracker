# M2 plan: run a real process, relay output

Unlike `m1-lessons-learned.md` (written after M1 was built and debugged),
this started as a plan before M2 was implemented. Core mechanism is now
built and verified working (2026-10-21): `Ping`/`Pong` handshake, then
`Exec` spawns a real process (`busybox`'s `echo`, baked into the rootfs as
a placeholder — see Dockerfile), `wait()`s on it, and returns its real exit
code. Output relay (below) is explicitly deferred, not part of this pass.

## Scope (from `ateom-firecracker-milestones.md`)

`guest-init` execs a hardcoded command and streams its stdout/stderr back
over vsock to the host. **Done when:** Firecracker boots, runs e.g. `echo
hello from the guest`, and the host process prints it. The command stays
hardcoded in `guest-init` for this milestone — "host tells guest what to
run" is explicitly later (M3 for the real OCI entrypoint, M5 for the
formalized protocol).

## Decisions made while planning

- **New RPC, not an overload of `Ping`.** Added `Exec`/`ExecResponse` to
  `api.proto` rather than folding this into the liveness check — keeps
  "are you alive" and "run something" as separate concerns, in line with
  where M5 is headed anyway. Built and working.
- **Reaping stays deferred to production** (already decided and recorded
  in M1/the milestones doc) — `guest-init` only `wait()`s the one spawned
  command for M2. Not revisited here.

## Deferred to a later milestone: real stdout/stderr relay

M2 landed with `Exec`'s child using `Stdio::inherit()` — its output reaches
the host only because it happens to flow through Firecracker's own serial
console, which `ateom-fc`'s `fcstarter.rs` already captures and logs. That
is **not** the host receiving output over the protocol; it's a side effect
of unrelated logging plumbing. Remove `inherit()` (switch to piped
`Stdio`) and the output disappears from the logs entirely until something
real reads it over ttrpc.

The actual design for that, decided but not built:

- **Polling, not buffer-then-return-everything.** Matches the real,
  already-verified `ateom-microvm` pattern: the host calls
  `ReadStdout`/`ReadStderr` repeatedly while the process runs, each call
  returning only what's accumulated since the last one — `guest-init` only
  ever holds a small, bounded chunk between polls, never the process's
  entire lifetime of output. (A true ttrpc streaming RPC was considered —
  ttrpc supports it as a first-class RPC kind, not a hack — but polling
  was chosen since it reuses the same request/response shape already
  working for `Ping`/`Exec`, rather than introducing a new RPC kind.)
- **The host-side polling loop is real, additional work** — not free once
  the RPC exists. Needs an interval loop calling `ReadStdout`/`ReadStderr`
  concurrently with a way to know the process is done (a `WaitProcess`-
  style call, or treating an RPC error/EOF as "done," matching
  `ateom-microvm` again).
- **Spawn + capture uses the same mechanism already proven in this
  codebase**: piped `Stdio` (`.stdout(Stdio::piped())`/`.stderr(...)`) on
  the child, same pattern `fcstarter.rs` already uses for Firecracker's own
  stdout/stderr — nothing new to invent on the guest side for this part.

## Bigger decision surfaced during planning: should `guest-init` be async at all?

Prompted by needing `async_trait` for the generated `GuestAgent` trait.
Conclusion: **no inherent need** — `guest-init` is a separate binary from
`ateom-fc`, so it doesn't inherit async from the host side just because
they share the `ateomfcpb` proto crate. Verified directly in `ttrpc`'s own
source that its **sync** API (the crate's own default feature set) fully
supports vsock transport too — vsock address parsing (`vsock://cid:port`)
lives in code shared by both sync and async (`common.rs`), not duplicated
or async-only.

Two concrete reasons favor switching `guest-init` to sync, beyond "fewer
dependencies":

1. **Negligible performance difference at this scale.** Blocking threads
   vs. tokio tasks only diverges meaningfully at high concurrency (thousands
   of tasks); `guest-init` supervises exactly one workload with a handful
   of threads total. If anything sync is a small net win here — no tokio
   runtime to initialize, smaller binary, which matters more on the
   boot-critical PID 1 path than usual.
2. **Zombie reaping is structurally simpler in sync.** Tokio's child-process
   support is built around "await the exit of a child *you* spawned via
   tokio" — it has no hook for reaping arbitrary reparented grandchildren,
   which is PID 1's actual job. Doing general reaping in tokio means either
   escaping to `spawn_blocking` for a raw `waitpid(-1, ...)` anyway, or
   handling `SIGCHLD` coalescing by hand inside a signal-handler loop. The
   sync version is the textbook 3-line pattern every minimal init
   (`tini`, `dumb-init`) already uses — `loop { waitpid(-1, None) }` until
   `ECHILD`. Since reaping is deferred, not blocking, but worth building
   the easy way whenever it does get built.

**Done.** `guest-init` moved to sync ttrpc (`ttrpc::sync::{Server,
TtrpcContext}`, plain `fn main()`, `std::thread::park()` as the sync
equivalent of the old `std::future::pending().await` "never exit"
mechanism) — `tokio`/`async-trait`/`tokio-vsock`/`futures` dropped from its
dependency tree entirely. `ateomfcpb`'s `build.rs` now generates two
independent copies of the bindings from one `api.proto` (`async_protos`
for `ateom-fc`, `sync_protos` for `guest-init`), each exposed as its own
module rather than fighting over one canonical path. Verified empirically,
not just in theory: the real linked `guest-init` binary shrank from ~1.9MB
to ~1.26MB after the switch.

## Open, not yet decided

- Real stdout/stderr relay (see above) — timing not yet decided.
