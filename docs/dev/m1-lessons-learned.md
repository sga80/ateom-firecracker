# M1 lessons learned: bare boot + liveness ping

M1's goal: boot a real Firecracker microVM end to end, with a custom Rust
`guest-init` as `/init`, and get a liveness handshake working between the
host (`ateom-fc`) and the guest over vsock. This document records what was
actually built, the real bugs hit getting there, and how each was found and
fixed — most of them were non-obvious and some are the kind that would
recur on a kernel/toolchain update, so they're worth keeping.

**Result:** working end to end. Host boots Firecracker, the guest kernel
boots off a minimal ext4 rootfs, `guest-init` mounts its filesystems, starts
a ttrpc server over vsock, and the host successfully calls `Ping` and gets
`Pong` back.

## What got built

- **`ateomfcpb`** — a new, shared workspace crate (not owned by either
  `ateom-fc` or `guest-init` individually) holding `api.proto`, the
  `ttrpc-codegen` `build.rs`, and the generated `api.rs`/`api_ttrpc.rs`.
  Both `ateom-fc` and `guest-init` depend on it via `path = "../ateomfcpb"`.
  Mirrors Substrate's own `ateompb`/`ateletpb` naming convention, since this
  project is meant to eventually integrate there. The service is
  `GuestAgent::Ping(PingRequest) -> PingResponse` — named to match this
  project's own established "liveness ping" terminology rather than a
  generic `Init`.
- **`ateom-fc/src/booter/`** — all 5 Firecracker boot-sequence API calls
  (`boot-source`, `drives`, `vsock`, `machine-config`, `actions`), each its
  own submodule, each following the same shape: a `#[serde(rename = ...)]`
  payload struct, a full-URL constant, and a method on `Booter` that PUTs
  the payload and delegates to a shared `handle_response` (checks for
  Firecracker's `204 No Content` success case).
- **`ateom-fc/src/virtual_sock.rs`** — host-side vsock: dials the UDS at
  `uds_path`, does the `CONNECT <port>\n` / `OK` handshake, then hands the
  resulting stream to a ttrpc client and calls `Ping`.
- **`guest-init/src/guest_agent.rs`** — guest-side vsock: binds a
  `tokio_vsock`/ttrpc `Listener` on `VMADDR_CID_ANY:8080`, implements the
  `GuestAgent` trait, serves it via `ttrpc::r#async::Server`.
- **`deploy/Dockerfile.ateom-fc` + `Makefile`** — builds both binaries and
  assembles the M1 rootfs entirely inside a real Linux container (no
  host-side `cargo build` at all); `make save-to-lima` and `make run-pod`
  drive the whole test loop with no manual steps left.

## Issues hit, and how each was actually found

### 1. Wrong `protobuf` crate pulled a C++ toolchain requirement

`protobuf = "4.36.2-release"` resolved to a completely different,
confusingly-named crate — a wrapper around Google's actual C++
`libprotobuf` via `cc`, not the pure-Rust `protobuf` crate (`3.x`) that
`ttrpc-codegen` itself already depends on. It needed a C cross-compiler
(`aarch64-linux-musl-gcc`) that didn't exist, which looked identical to an
earlier, unrelated cross-compiler error from `reqwest`'s `aws-lc-sys`.
**Found by:** reading `Cargo.lock` directly and comparing the two
`protobuf` entries side by side. **Fixed by:** repinning to `protobuf =
"3.7"`, matching what `ttrpc-codegen` already resolves to.

### 2. `build.rs` in the wrong place

A build script needs to be a file literally named `build.rs` at the crate
root — Cargo detects it by that fixed name/location, not by any `mod`
declaration, and `[build-dependencies]` are only ever linked into that
separate build-script binary, never into the crate's own normal
compilation. An earlier `src/proto_build.rs`, pulled in via `mod
proto_build;`, got compiled as ordinary crate code instead, which can't see
`ttrpc_codegen` at all. **Found by:** the real compiler error
(`E0433`/unresolved crate) pointed straight at it. **Fixed by:** moving the
file to `build.rs` at the crate root and deleting the `mod` line.

### 3. `ttrpc`'s `"async"` feature isn't the default, and enabling it still wasn't enough

`ttrpc`'s own `[features]` table has `default = ["sync", "rustprotobuf"]` —
`"async"` has to be requested explicitly, or the generated code (built
with `Customize { async_all: true, .. }`) can't compile at all. Even after
adding it, a second, subtler gap remained: `async-trait` is only pulled in
as *ttrpc's own* transitive dependency under that feature — it's present
in the build, but not usable via `use async_trait::async_trait` from
`ateomfcpb`'s own generated code, because Cargo only lets a crate `use`
something listed in *its own* `[dependencies]`. A transitive dependency
being part of the overall build is not the same as it being importable.
**Found by:** the real `E0432` unresolved-import error, after first
assuming (wrongly) that enabling `ttrpc`'s feature alone would be enough.
**Fixed by:** adding `async-trait` as a direct dependency of `ateomfcpb`
too.

### 4. Docker's read-only bind mount vs. codegen writing into `src/`

`ateomfcpb/build.rs` generates code directly into its own `src/protos/`
(not `OUT_DIR`). That's fine on a normal host build, but Docker's
`--mount=type=bind,source=.,target=/app` mounts the build context
read-only by default, so the containerized build failed with "Read-only
file system" the first time it was actually run inside the Dockerfile.
**Found by:** actually running `docker build` and reading the real error,
not assuming the Dockerfile would work because the syntax looked right.
**Fixed by:** making the mount explicitly `,rw`.

### 5. `devtmpfs` `EBUSY` on `/dev`

`guest-init`'s own explicit `mount("dev", "/dev", "devtmpfs", ...)` failed
with `EBUSY`. Root cause: the fetched `vmlinux`'s kernel config has
`CONFIG_DEVTMPFS_MOUNT=y`, which makes the *kernel itself* auto-mount
devtmpfs at `/dev` before `init` ever runs — so by the time `guest-init`
tried its own explicit mount, something was already there. **Found by:**
extracting the kernel's own embedded config directly from the `vmlinux`
binary (kernels built with `CONFIG_IKCONFIG` embed it, bounded by
`IKCFG_ST`/`IKCFG_ED` markers, gzip-compressed in between) and grepping
for `DEVTMPFS` — a real, checkable fact, not a guess from general kernel
knowledge.

### 6. Missing `init=/init` boot arg

The kernel panicked with "No working init found. Try passing init= option
to kernel," and fell back to trying `/bin/sh` (which doesn't exist in this
minimal rootfs). The kernel's default init search (`/sbin/init`,
`/etc/init`, `/bin/init`, `/bin/sh`) never includes plain `/init` on a
direct, non-initramfs rootfs boot — that convention only applies
automatically in an initramfs scenario. Since `guest-init` lives at
`/init`, it was never even tried. **Fixed by:** adding `init=/init` to
`BOOT_ARGS`.

### 7. PID 1 must never exit, even successfully

Once mounting and the boot args were fixed, the kernel panicked again:
"Attempted to kill init!" — because `main()` returned `Ok(())` right after
starting the ttrpc server, and in Linux, **PID 1 exiting for any reason,
including success, is fatal**; there's nothing to fall back to for an
entire kernel. **Fixed, for now, with `std::future::pending::<()>().await`
as the last line of `main()`** — a real stopgap, not the final design; the
real version of this should fold into the init's zombie-reaping
responsibility (still not built). Worth noting this isn't unique to
`guest-init`: `ateom-fc` is PID 1 of its own *container* (not the guest
kernel) for the same underlying reason — if its `main()` ever returns, the
container's PID namespace tears down and kills the `firecracker` child
process (and the VM) with it. Not yet addressed on that side.

### 8. The big one: `ttrpc::r#async::Server::start()` doesn't block, and a dropped temporary killed the listener instantly

Even after the above fixes, the host's vsock handshake failed consistently
— `CONNECT 8080\n` always got back a flat empty response (`EOF`), every
single retry, no matter how long the retry loop waited. This looked like a
boot-order race at first, but it wasn't: `guest-init` was confirmed
already listening (logged `started ttrpc server`) well before the host's
first attempt, and it still failed every time — a persistent problem, not
a timing one.

Root cause, found by reading `ttrpc`'s actual source rather than assuming
its API's behavior: `Server::start()` spawns the connection-accept loop as
a **background tokio task** and returns `Ok(())` immediately — it does not
block for the life of the process. The original code built the `Server` as
a temporary, inline in a chained expression
(`Server::new()...start().await`), never binding it to a variable. The
instant that statement finished, the `Server` — including
`stop_listen_tx`, the shutdown-signal sender it owns — got dropped. A
dropped `mpsc::Sender` makes the corresponding `.recv()` resolve to `None`
almost immediately, and the spawned accept loop's `select!` has an
**unconditional** `break` on that branch (the `break` sits outside the `if
let Some(...)`, so it runs whether `recv()` got a real message or just
"channel closed"). So the accept loop task exited within moments of being
spawned — long before any real `CONNECT` attempt could ever reach it. This
explains the symptom precisely: not a race that more retries would fix,
but a listener that was already dead by the time anything tried to use it.

**Fixed by:** binding the `Server` to a variable that stays alive for the
life of the process (`let mut server = get_ttrpc_server().await?;` kept in
scope through the `pending()` call in `main()`). A second, compounding bug
surfaced in the same area during the fix: `.register_service(methods)` was
initially never called at all, so even with the lifetime corrected, no
`Ping` handler was actually registered on the server.

## Still open after M1

- `ateom-fc`'s own "never exit" mechanism (same PID-1-in-a-container class
  of bug as #7, not yet fixed on the host side).
- Replacing the throwaway M1 rootfs (`guest-init` as the only file at
  `/init`, nothing else) with the real per-actor OCI-image pipeline
  (`internal/rootfs`, M3 — Substrate's `internal/imagecache` already does
  the expensive pull/unpack/cache work; the missing piece is turning a
  composed overlay directory into something Firecracker can attach as a
  drive).
- Zombie reaping in `guest-init` (a real init responsibility, still not
  implemented).
- `start_connect`'s retry loop (added while suspecting a boot race that
  turned out not to be the actual bug) — harmless and still worth keeping
  as a defensive measure, but wasn't what fixed anything here.
