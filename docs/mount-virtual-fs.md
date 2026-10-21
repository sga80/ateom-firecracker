# Mounting `/proc`, `/sys`, `/dev` in `guest-init`

One of `guest-init`'s first jobs, before anything else can reliably function
inside the guest: mounting the kernel's virtual filesystems. Unlike the root
filesystem (`rootfs.ext4`, supplied via `drives` — see `docs/drives.md`),
these aren't build-time artifacts at all; they're live views into a running
kernel, and only `guest-init` itself, at runtime, can bring them into
existence.

## Why this is `guest-init`'s job, not the build pipeline's

Everything else in the rootfs staging directory is genuinely static: the
`guest-init` binary is a fixed set of bytes, known in advance, safe to bake
into `rootfs.ext4` once, ahead of time. `/proc` and `/sys` are not static —
they have no bytes to pre-compute at all. `/proc/meminfo`'s "content" isn't
a file sitting somewhere; it's the kernel generating text on the spot,
reflecting whatever is true about the system at the exact instant it's
read. There is no fixed answer to "what does `/proc/meminfo` contain" the
way there's a fixed answer to "what bytes make up the `guest-init` binary."

Mounting is a `mount()` **syscall** — it says "attach a live filesystem
driver here, for *this specific, currently-executing kernel instance*,
right now." That syscall can only be issued by a process running under a
kernel that is actually alive. At build time (when `rootfs.ext4` is
assembled via `mkfs.ext4 -d`), the guest kernel doesn't exist yet — it
hasn't booted, so there's nothing for `/proc`/`/sys` to possibly reflect.

So the build pipeline's job is only to create the **empty directories**
(`/proc`, `/sys`, `/dev` as plain folders — valid mount *points*, since
`mount()` needs an existing directory as its target). The actual *mount*
action — the live redirect — can only happen once `guest-init` is already
running as PID 1 inside the booted guest.

## What `mount()` actually does, mechanically

Before mounting, `/proc` is an ordinary, empty directory — a real entry on
the `rootfs.ext4` filesystem, same as any other folder. The kernel
maintains an internal table (part of the VFS — virtual filesystem layer)
saying, for every path, which filesystem driver owns it. Before the
`mount()` call, `/proc` is owned by plain ext4; any lookup under that path
resolves to ext4's (empty) on-disk data.

`mount("proc", "/proc", "proc", ...)` adds a new entry to that table: *from
now on, anything looked up under `/proc` is redirected to the `procfs`
driver instead of ext4*. The original empty ext4 directory isn't deleted —
it's just permanently shadowed as long as something is mounted on top of
it. Every subsequent `open("/proc/meminfo")`, from any process, gets
intercepted by the kernel's path-resolution logic before it ever reaches
ext4, and handed to `procfs`'s own code instead — which generates the
content live from the kernel's actual current state.

Without the `mount()` call, none of this redirection exists: `/proc` stays
exactly what it is on disk — an empty folder — and `open("/proc/meminfo")`
just fails with "no such file," because nothing was ever placed there.

## The `mount()` syscall's parameters

```c
int mount(const char *source, const char *target,
          const char *filesystemtype, unsigned long mountflags,
          const void *data);
```

- **`target`** — *where* the mount attaches: the path to the (empty) mount
  point, e.g. `/proc`.
- **`fstype`** — *which driver* handles this path from now on. This is the
  parameter doing the real work for a virtual filesystem — it's what
  actually selects `procfs`/`sysfs`/`devtmpfs`.
- **`source`** — meaningful for a *real* filesystem (the block device or
  file containing actual data — the same role `path_on_host` plays in the
  `drives` API call). For a virtual filesystem, there's no real data at
  all, so this is effectively vestigial: the kernel ignores it. The
  convention of passing the fstype's own name (e.g. `"proc"`) is purely a
  human-readable label — what shows up in `/proc/mounts`, not something the
  kernel uses to find or interpret data.

## Calling it from Rust

No standard-library wrapper exists for this — `std::fs` has no concept of
mounting, far too platform-specific and privileged for Rust's portable-by-design
std. The options are the `libc` crate (raw FFI, `unsafe`, matches the
project's house convention of minimizing dependencies especially on the
guest side — see `AGENTS.md`-style reasoning in the project's working
conventions) or `nix` (a safer, more ergonomic wrapper around `libc`, but a
broader dependency surface). This project uses `libc` directly.

The real signature, confirmed directly from `libc` v0.2.191's own source
(`src/unix/linux_like/linux_l4re_shared.rs`) — **5 arguments**:

```rust
pub fn mount(
    src: *const c_char,
    target: *const c_char,
    fstype: *const c_char,
    flags: c_ulong,
    data: *const c_void,
) -> c_int;
```

```rust
use std::ffi::CString;
use std::io;

fn mount_virtual_fs(name: &str, fstype: &str, flags: libc::c_ulong) -> io::Result<()> {
    let source = CString::new(name).unwrap();
    let target = CString::new(format!("/{name}")).unwrap();
    let fstype = CString::new(fstype).unwrap();

    let result = unsafe {
        libc::mount(
            source.as_ptr(),
            target.as_ptr(),
            fstype.as_ptr(),
            flags,
            std::ptr::null(),
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
```

`CString::new(...)` does the C-string conversion the raw syscall requires
(NUL-terminated, unlike Rust's own `String`/`&str`). `io::Error::last_os_error()`
reads `errno` after a failed call and wraps it as a real, descriptive Rust
error — the equivalent of checking `errno` by hand after any POSIX syscall.

## A real gotcha: `cargo build`/`cargo check` with no `--target` proves nothing

Plain `cargo build` compiles natively for the **host** — e.g. `aarch64-apple-darwin`
on this Mac — not for the Linux guest this code actually runs in. macOS's
`mount()` is BSD-flavored and has a **different signature**:
`mount(type, dir, flags, data)` — 4 arguments, no separate `source` at all.
Code that compiles cleanly against Darwin's `libc::mount` can be silently
wrong against Linux's 5-argument version — "it compiles" proved nothing
about the actual target.

The fix: `rustup target add aarch64-unknown-linux-musl`, then use
`cargo check --target aarch64-unknown-linux-musl` while iterating. `cargo check`
stops before linking, so it catches real target-specific signature/type
errors without needing a working musl cross-linker set up at all. (A full
*build* — producing an actual linked binary — still needs the real
cross-compile toolchain; see the project's build-pipeline notes for the
Docker-based approach modeled on `ironlog`.)

## The three real calls, and their exact `fstype` strings

Verified directly against a real, running kernel's own registered
filesystem list (`cat /proc/filesystems` inside `fcvm`), not assumed from
memory:

```
nodev	sysfs
nodev	proc
nodev	devtmpfs
```

`"procfs"` is the common *prose* name — what people say when talking about
it — but the kernel's actually-registered type string is the short form
`proc`. `proc` is the one case where directory name and `fstype` happen to
coincide; `sysfs` and `devtmpfs` don't shorten to match their mount points
(`sys`, `dev`).

```rust
mount_virtual_fs("proc", "proc", 0)?;
mount_virtual_fs("sys", "sysfs", libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC)?;
mount_virtual_fs("dev", "devtmpfs", libc::MS_NOSUID)?;
```

## The mount flags, and what each one actually does

Also verified against a real system rather than assumed — `fcvm`'s own
`/proc/self/mountinfo` shows a standard Ubuntu/systemd boot using:

```
/dev  →  rw,nosuid
/sys  →  rw,nosuid,nodev,noexec,relatime
```

Notably **not** read-only for either — these flags narrow specific
*capabilities* a file could have, not general write access. None of them
are strictly required for basic functionality (`flags: 0` would still
work) — they're hardening, matching a real init system's defaults rather
than a hard technical requirement the way the `fstype` strings are.

- **`MS_NOSUID`** — disables the **setuid** mechanism for anything on this
  filesystem. Normally, a binary with the setuid bit set (the classic
  example: `/usr/bin/passwd`, owned by `root`) runs as its *owner*, not as
  whoever executed it — a deliberate, narrow way to let unprivileged users
  trigger one specific privileged operation through a trusted program.
  `nosuid` tells the kernel to ignore that bit entirely for this
  filesystem — even if a malicious setuid-root binary somehow ended up
  here, it couldn't actually gain elevated privilege by being executed.
  Free to apply to `/sys`/`/dev`: nothing legitimate on either ever needs
  setuid behavior.

- **`MS_NODEV`** — disables treating anything on this filesystem as a real
  **device node**. A device file (like `/dev/kvm`) doesn't hold real file
  content — opening it routes the request to a kernel device driver
  instead, keyed by a major:minor device number. `nodev` means: even if a
  file here has device-node attributes, never honor them — opening it just
  acts as an inert, useless file. Free to apply to `/sys`: its content is
  kernel-generated metadata, never a place a real device node should
  legitimately exist.

- **`MS_NOEXEC`** — disables **executing** anything on this filesystem,
  regardless of the individual file's own permission bits. A process can
  still read a file here, but `exec()`-ing it as a program is refused
  outright. Free to apply to `/sys`: it holds metadata, not programs.

Applying all three to `/sys`, and just `nosuid` to `/dev` (which, unlike
`/sys`, genuinely needs to stay writable and executable, since real device
nodes get created there dynamically), matches exactly what a real
Ubuntu/systemd boot does by default — a well-tested baseline worth matching
rather than reinventing.
