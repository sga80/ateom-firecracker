# Builds both ateom-fc and guest-init for the Lima/kind target (linux/arm64,
# matching this Mac's Apple Silicon — both Docker Desktop and the fcvm Lima
# VM run arm64 natively, so no emulation either way). Ported from the old
# Go-based Makefile at ~/code/go/src/ateom-firecracker/Makefile — guest-init
# now has real source (it didn't when that Makefile was written), so it's
# built here too.
#
# No host-side `cargo build` targets here at all — macOS has no working
# aarch64-unknown-linux-musl cross-linker out of the box, so a real link
# (not just `cargo check`) fails on this Mac. Everything compiles inside
# deploy/Dockerfile.ateom-fc's build stage instead (a real Linux container,
# same pattern already used in ~/code/rust/projects/ironlog) — no
# cross-linker needed there at all, since the compiler and the target match.
#
# TARGET is still used here (for FC_ARCH below), matching .cargo/config.toml.
# musl gives a static, libc-free binary — the same "no libc inside the
# distroless image" property Go's CGO_ENABLED=0 gave the old build.

TARGET ?= aarch64-unknown-linux-musl

BUILD_DIR := build

ASSETS_DIR := assets
FC_VERSION ?= v1.13.1
# The firecracker-ci kernel bucket is keyed by major.minor only (e.g.
# "v1.13"), not the full patch version — derive it from FC_VERSION so there's
# one version number to bump, not two that could drift apart.
FC_VERSION_MINOR := $(basename $(FC_VERSION))
# Firecracker's own release naming ("aarch64"/"x86_64") already matches the
# first segment of a Rust target triple — no GOARCH-style mapping needed,
# unlike the old Go build.
FC_ARCH := $(firstword $(subst -, ,$(TARGET)))

IMAGE_NAME ?= localhost/ateom-fc
IMAGE_TAG ?= dev

TAG_AS ?=

LIMA_INSTANCE ?= fcvm
KIND_CLUSTER ?= ateom-fc
# IMAGE_NAME can contain a literal "/" (e.g. localhost/ateom-fc) — sanitize
# it before using it as part of a filename, or that "/" reads as a path
# separator instead of part of the name (build/localhost/ateom-fc-dev.tar,
# which docker save won't auto-create the directory for).
IMAGE_FILE_SAFE_NAME := $(subst /,-,$(IMAGE_NAME))
IMAGE_ARCHIVE := $(BUILD_DIR)/$(IMAGE_FILE_SAFE_NAME)-$(IMAGE_TAG).tar
LIMA_ARCHIVE_PATH := /tmp/$(IMAGE_FILE_SAFE_NAME)-$(IMAGE_TAG).tar

POD_SPEC := k8s-resources/pod.yaml
LIMA_POD_SPEC_PATH := /tmp/ateom-fc-pod.yaml

.PHONY: all image fetch-assets clean rebuild tag save-to-lima run-pod

all: image

# Does exactly what it says, in order: wipe build/, make sure firecracker is
# fetched, then rebuild the image (which compiles both binaries itself,
# inside its own Dockerfile build stage). Each step runs as its own
# `$(MAKE)` call so the order is guaranteed regardless of parallel make
# flags (`-j`), unlike plain prerequisite lists.
rebuild:
	$(MAKE) clean
	$(MAKE) fetch-assets
	$(MAKE) image

# Idempotent: skips each download entirely if its target is already there,
# so repeat runs (e.g. as part of a wider build) don't re-fetch.
fetch-assets:
	@if [ -f $(ASSETS_DIR)/firecracker ]; then \
		echo "$(ASSETS_DIR)/firecracker already present, skipping fetch"; \
	else \
		mkdir -p $(ASSETS_DIR); \
		tmp=$$(mktemp -d); \
		curl -fL "https://github.com/firecracker-microvm/firecracker/releases/download/$(FC_VERSION)/firecracker-$(FC_VERSION)-$(FC_ARCH).tgz" -o "$$tmp/firecracker.tgz"; \
		tar -xzf "$$tmp/firecracker.tgz" -C "$$tmp"; \
		install -m 0755 "$$tmp/release-$(FC_VERSION)-$(FC_ARCH)/firecracker-$(FC_VERSION)-$(FC_ARCH)" $(ASSETS_DIR)/firecracker; \
		rm -rf "$$tmp"; \
	fi
	@if [ -f $(ASSETS_DIR)/vmlinux ]; then \
		echo "$(ASSETS_DIR)/vmlinux already present, skipping fetch"; \
	else \
		mkdir -p $(ASSETS_DIR); \
		kkey=$$(curl -fsSL "https://s3.amazonaws.com/spec.ccfc.min?prefix=firecracker-ci/$(FC_VERSION_MINOR)/$(FC_ARCH)/vmlinux-5.10&list-type=2" \
			| grep -oE "firecracker-ci/$(FC_VERSION_MINOR)/$(FC_ARCH)/vmlinux-5\.10\.[0-9]+" | sort -V | tail -1); \
		if [ -z "$$kkey" ]; then echo "could not find a vmlinux build for $(FC_VERSION_MINOR)/$(FC_ARCH) in the firecracker-ci bucket" >&2; exit 1; fi; \
		curl -fL "https://s3.amazonaws.com/spec.ccfc.min/$$kkey" -o "$(ASSETS_DIR)/vmlinux"; \
	fi

# Unlike the old Go Dockerfile, this one compiles both binaries itself
# inside its own build stage (see deploy/Dockerfile.ateom-fc) — no local
# `cargo build` step needed or provided here at all.
image:
	docker build -f deploy/Dockerfile.ateom-fc -t $(IMAGE_NAME):$(IMAGE_TAG) .

clean:
	rm -rf $(BUILD_DIR)

# Retags the already-built image under a different name — e.g. a registry
# path, or whatever tag another daemon/tool (kind, a separate Docker inside
# fcvm) expects. Doesn't push anywhere by itself; this only adds the second
# tag in whatever local image store `image` just built into.
tag: image
	@if [ -z "$(TAG_AS)" ]; then \
		echo "set TAG_AS=<new-tag>, e.g. make tag TAG_AS=localhost:5000/ateom-fc:dev" >&2; \
		exit 1; \
	fi
	docker tag $(IMAGE_NAME):$(IMAGE_TAG) $(TAG_AS)

# Exports the already-built image from this Mac's local Docker (Desktop) and
# copies it into the fcvm Lima VM as a plain file — via `limactl copy`, which
# needs no shared-mount config, unlike our earlier approach for this repo's
# source. Lands at $(LIMA_ARCHIVE_PATH) inside the VM.
#
# Also loads it straight into fcvm's own Docker and into the `ateom-fc` kind
# cluster's containerd (`kind load docker-image`), since Docker/kind are
# already installed there — no more manual docker load/kind load steps.
save-to-lima: image
	mkdir -p $(BUILD_DIR)
	docker save $(IMAGE_NAME):$(IMAGE_TAG) -o $(IMAGE_ARCHIVE)
	limactl copy $(IMAGE_ARCHIVE) $(LIMA_INSTANCE):$(LIMA_ARCHIVE_PATH)
	limactl shell $(LIMA_INSTANCE) -- docker load -i $(LIMA_ARCHIVE_PATH)
	limactl shell $(LIMA_INSTANCE) -- kind load docker-image $(IMAGE_NAME):$(IMAGE_TAG) --name $(KIND_CLUSTER)
	@echo "loaded into $(LIMA_INSTANCE)'s '$(KIND_CLUSTER)' kind cluster — ready for kubectl apply"

# Copies k8s-resources/pod.yaml into fcvm and applies it. Deletes any
# existing ateom-fc pod first (--ignore-not-found, so a first run is fine
# too) — Pod specs are mostly immutable, so for dev iteration a fresh pod
# each run is simpler than fighting `kubectl apply` over in-place updates.
run-pod:
	limactl copy $(POD_SPEC) $(LIMA_INSTANCE):$(LIMA_POD_SPEC_PATH)
	limactl shell $(LIMA_INSTANCE) -- kubectl delete -f $(LIMA_POD_SPEC_PATH) --ignore-not-found
	limactl shell $(LIMA_INSTANCE) -- kubectl apply -f $(LIMA_POD_SPEC_PATH)
