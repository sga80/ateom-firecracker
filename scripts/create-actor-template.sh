#!/usr/bin/env bash
# Resolves k8s-resources/actortemplate.yaml's ko:// image reference and
# creates it as an ActorTemplate via ateapi's control API. Run this inside
# fcvm (where ko/kubectl/kubectl-ate and the kind cluster context already
# live), from anywhere -- paths below are absolute.
#
# ActorTemplate is not a CRD, so it can't go through `kubectl apply`: it's a
# protojson-shaped ateapipb.ActorTemplate, created through the kubectl-ate
# plugin, which talks to ateapi's gRPC control API directly. `ko resolve`
# only replaces the ko:// image reference in the YAML text; the
# ${BUCKET_NAME} placeholder is plain shell substitution via sed, same as
# substrate's own install-microvm-deps.sh does for its SandboxConfig.
#
# The SandboxConfig and WorkerPool ARE real CRDs, so they go through a plain
# `kubectl apply` first -- the ActorTemplate's workerSelector/sandboxConfig
# only resolve once both of those already exist in the cluster.

set -o errexit -o nounset -o pipefail

SUBSTRATE_DIR="${SUBSTRATE_DIR:-/Users/shyamgavulla/code/go/src/substrate}"
K8S_RESOURCES_DIR="${K8S_RESOURCES_DIR:-/Users/shyamgavulla/code/rust/projects/ateom-firecracker/k8s-resources}"
TEMPLATE_FILE="${TEMPLATE_FILE:-${K8S_RESOURCES_DIR}/actortemplate.yaml}"

kubectl apply -f "${K8S_RESOURCES_DIR}/sandboxconfig.yaml"
kubectl apply -f "${K8S_RESOURCES_DIR}/workerpool.yaml"

# Matches install-ate-kind.sh's defaults for this kind cluster.
export BUCKET_NAME="${BUCKET_NAME:-ate-snapshots}"
export KO_DOCKER_REPO="${KO_DOCKER_REPO:-localhost:5001}"

cd "${SUBSTRATE_DIR}"

# ko is not a plain installed binary here -- it's pinned as a Go tool in
# hack/tools/ko. hack/run-tool.sh resolves it (its default case is
# hack/tools/<tool-name>, which matches ko) via `go tool -n ko`, then execs
# it from the original CWD (the repo root, via the `cd` above), so import
# paths like github.com/agent-substrate/substrate/demos/counter resolve
# against the main module -- same mechanism as every other pinned tool in
# this repo (kind, protoc-gen-go, ...).
KO_CMD=(./hack/run-tool.sh ko)

# kubectl-ate may not be on PATH yet -- fall back to `go run` if so, same
# idea as hack/run-tool.sh does for other pinned tools elsewhere in this repo.
if command -v kubectl-ate >/dev/null 2>&1; then
  ATE_CMD=(kubectl ate)
else
  echo "kubectl-ate not on PATH, falling back to 'go run ./cmd/kubectl-ate'" >&2
  ATE_CMD=(go run ./cmd/kubectl-ate)
fi

sed -e "s|\${BUCKET_NAME}|${BUCKET_NAME}|g" "${TEMPLATE_FILE}" \
  | "${KO_CMD[@]}" resolve -f - \
  | "${ATE_CMD[@]}" create actor-template -f -
