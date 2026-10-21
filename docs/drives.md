# `PUT /drives/{drive_id}`

Step 2 of 5 in the Firecracker boot sequence. Supplies the root filesystem —
the other half of what a guest needs to boot (the kernel is supplied
separately by `boot-source`, see `docs/boot-source.md`).

## What a "drive" actually is

The kernel configured via `boot-source` is just the kernel — no files, no
`/bin/bash`, no `/init`, nothing. The moment it starts running it needs to
**mount something as its root filesystem (`/`)**, or it has nothing to
execute at all and panics almost immediately ("no root filesystem found").

A "drive," in Firecracker's model, is exactly that something: a **virtual
block device** — the microVM equivalent of a real disk — backed by an
actual file sitting on the host. That file is `rootfs.ext4`, which we build
separately (see below). From the guest kernel's point of view, this looks
exactly like a real disk it can mount.

So: `boot-source` supplies the *kernel*; `drives` supplies the *filesystem
that kernel runs from*. Both are required before `actions`/`InstanceStart`
can succeed — missing either means there's nothing for the guest to boot
into.

## API shape — the one wrinkle worth knowing

Unlike `boot-source`, this endpoint is `PUT /drives/{drive_id}` — the
drive's ID is part of the **URL itself**, and it also has to appear again in
the body. Both have to be the exact same string:

```
PUT /drives/rootfs
```
```json
{
  "drive_id": "rootfs",
  "path_on_host": "/rootfs.ext4",
  "is_root_device": true,
  "is_read_only": false
}
```

If you name the drive `"rootfs"`, the URL must be `PUT /drives/rootfs` *and*
the body's `drive_id` field must say `"rootfs"` — not a different string in
either place. If they don't match, Firecracker rejects the request with a
`400`. This isn't Firecracker-specific — it's a common REST pattern: the URL
identifies *which* resource you're operating on, the body is the full
representation of it, and the ID is deliberately repeated in both so the API
can validate they agree (a sanity check against copy-paste mistakes).

## Payload fields

| Field | Meaning |
|---|---|
| `drive_id` | A string you choose to name this drive (e.g. `"rootfs"`) — must match the URL |
| `path_on_host` | The actual file path to `rootfs.ext4`, inside `ateom-fc`'s own container (same filesystem as `kernel_image_path` — see `docs/boot-source.md`) |
| `is_root_device` | `true` — mount this one as `/`, as opposed to an extra data volume |
| `is_read_only` | `false` — the guest needs to write to its own root filesystem (create files, write logs, etc.) |

For M1 specifically: one single, writable rootfs drive. The read-only
shared-base-plus-writable-overlay split (multiple drives, one shared across
actors, one per-actor) that real Substrate's `ateom-microvm` gets via
virtio-fs is explicitly **M3 territory** — not needed here.

## Building the actual file is not Go code

Constructing `rootfs.ext4` is a **build-time** step, not something
`ateom-fc`'s Go code does at runtime — same category as `vmlinux` and the
`firecracker` binary itself: a file that has to already exist inside the
`ateom-fc` container's filesystem before the container ever starts.

The mechanism (a script/Makefile target, not Go):
1. Stage a built `ateom-init` binary as `/init` in an otherwise-empty
   directory, alongside empty `/proc`, `/sys`, `/dev` mount points.
2. `truncate` a file to the desired size.
3. `mkfs.ext4 -d <staging-dir> -F rootfs.ext4` — formats the file as ext4
   and seeds it from the staging directory in one step.
4. `COPY` the result into the Dockerfile, same as `firecracker` and
   `vmlinux`.

`drives()`'s whole job in Go is to take a path that already points at a real,
pre-built file and tell Firecracker about it over the API — it doesn't build
or touch the filesystem itself, exactly like `bootSource()` takes
`kernelPath` without knowing or caring how `vmlinux` got there.

## Response

Same as every config call: `204 No Content` on success, `400` with
`{"fault_message": "<string>"}` on failure.
