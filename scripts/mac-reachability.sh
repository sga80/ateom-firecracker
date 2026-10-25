#!/usr/bin/env bash
# Throwaway test scaffolding - NOT part of ateom-fc's real code, and not
# something that survives Substrate integration (real atenet owns this
# problem in production). This exists purely so you can curl the counter
# demo from your Mac during M4 development, through the full chain:
#
#   Mac -> Lima's auto-forwarded port -> kubectl port-forward -> socat in
#   the pod's own netns -> veth pair -> DNAT in the actor's netns -> tap0
#   -> Firecracker -> guest eth0
#
# Run this FROM INSIDE the Lima VM (`limactl shell fcvm`), after a fresh
# actor has booted and its counter demo is listening on 169.254.17.2:80.
# Safe to re-run: tears down anything left over from a previous run first.

set -euo pipefail

KIND_NODE="${KIND_NODE:-ateom-fc-control-plane}"
POD_NAME="${POD_NAME:-ateom-fc}"
VETH_SUBNET_HOST="10.200.0.1/30"
VETH_SUBNET_ACTOR="10.200.0.2/30"
GUEST_ADDR="169.254.17.2"
GUEST_PORT="80"
LOCAL_PORT="8080"

node_exec() {
    docker exec "$KIND_NODE" bash -lc "$1"
}

echo "--- tearing down anything left over from a previous run ---"
pkill -f "kubectl port-forward pod/$POD_NAME" 2>/dev/null || true
node_exec "pkill -f 'socat TCP-LISTEN:$LOCAL_PORT' 2>/dev/null || true"
node_exec "ip link show veth-host >/dev/null 2>&1 && ip link del veth-host || true"
sleep 1

echo "--- finding the ateom-fc container pid and firecracker pid on $KIND_NODE ---"
CID=$(node_exec "crictl ps --name $POD_NAME -q" | head -1)
if [[ -z "$CID" ]]; then
    echo "no running container named $POD_NAME found - is the pod up?" >&2
    exit 1
fi
POD_PID=$(node_exec "crictl inspect $CID" | grep -m1 '"pid"' | grep -o '[0-9]*')
FC_PID=$(node_exec "ps aux | grep '[f]irecracker --api-sock'" | awk '{print $2}' | head -1)
if [[ -z "$POD_PID" || -z "$FC_PID" ]]; then
    echo "could not resolve pod pid ($POD_PID) or firecracker pid ($FC_PID)" >&2
    exit 1
fi
echo "pod pid (host-visible) = $POD_PID"
echo "firecracker pid (host-visible, anchors the actor's netns) = $FC_PID"

echo "--- creating veth pair: veth-host (pod netns) <-> veth-actor (actor netns) ---"
node_exec "nsenter --target $POD_PID --net ip link add veth-host type veth peer name veth-actor"
node_exec "nsenter --target $POD_PID --net ip link set veth-actor netns $FC_PID"

node_exec "nsenter --target $POD_PID --net ip addr add $VETH_SUBNET_HOST dev veth-host"
node_exec "nsenter --target $POD_PID --net ip link set veth-host up"

node_exec "nsenter --target $FC_PID --net ip addr add $VETH_SUBNET_ACTOR dev veth-actor"
node_exec "nsenter --target $FC_PID --net ip link set veth-actor up"

echo "--- forwarding + NAT inside the actor's netns ---"
node_exec "nsenter --target $FC_PID --net sysctl -w net.ipv4.ip_forward=1 >/dev/null"
node_exec "nsenter --target $FC_PID --net iptables -t nat -C PREROUTING -i veth-actor -p tcp --dport $LOCAL_PORT -j DNAT --to-destination $GUEST_ADDR:$GUEST_PORT 2>/dev/null || \
           nsenter --target $FC_PID --net iptables -t nat -A PREROUTING -i veth-actor -p tcp --dport $LOCAL_PORT -j DNAT --to-destination $GUEST_ADDR:$GUEST_PORT"
node_exec "nsenter --target $FC_PID --net iptables -t nat -C POSTROUTING -o tap0 -j MASQUERADE 2>/dev/null || \
           nsenter --target $FC_PID --net iptables -t nat -A POSTROUTING -o tap0 -j MASQUERADE"

echo "--- starting socat proxy (127.0.0.1:$LOCAL_PORT -> veth-actor peer) inside the pod's own netns ---"
node_exec "which socat >/dev/null 2>&1 || (apt-get update -qq && apt-get install -y -qq socat)"
node_exec "nsenter --target $POD_PID --net setsid socat TCP-LISTEN:$LOCAL_PORT,fork,reuseaddr TCP:10.200.0.2:$LOCAL_PORT </dev/null >/tmp/socat-proxy.log 2>&1 & disown"
sleep 1

echo "--- starting kubectl port-forward (0.0.0.0:$LOCAL_PORT -> pod's $LOCAL_PORT) in Lima ---"
nohup kubectl port-forward "pod/$POD_NAME" "$LOCAL_PORT:$LOCAL_PORT" --address 0.0.0.0 >/tmp/port-forward.log 2>&1 &
disown
sleep 2
cat /tmp/port-forward.log

echo ""
echo "Setup complete. From your Mac's own terminal, run:"
echo "  curl 127.0.0.1:$LOCAL_PORT"
