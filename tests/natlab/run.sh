#!/usr/bin/env bash
# NAT lab: tests beemr across real Linux NATs, entirely inside Docker.
#
#   lan-a (10.10.1.0/24)              "internet" (172.30.0.0/24)              lan-b (10.10.2.0/24)
#   host-a ── nat-a (MASQUERADE) ──┬── relay (beemr relay) ──┬── nat-b (MASQUERADE) ── host-b
#                                  └── dht-1, dht-2 (private DHT) ┘
#
# Usage: tests/natlab/run.sh <cone|symmetric> [--no-relay]
#   cone       NATs keep the same external port per socket: hole punching should succeed.
#   symmetric  NATs pick a random port per destination: hole punching fails, the relay carries data.
#   --no-relay no relay on the internet: transfers must fail with a clear explanation.
#
# Needs a static Linux build of beemr; set BIN to its path (default:
# target/x86_64-unknown-linux-musl/release/beemr). CI runs this on every push.
set -euo pipefail

MODE="${1:-cone}"
WITH_RELAY=1
[[ "${2:-}" == "--no-relay" ]] && WITH_RELAY=0
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BIN="${BIN:-$ROOT/target/x86_64-unknown-linux-musl/release/beemr}"
IMAGE="bplab-node"
P="bplab"   # resource prefix
[[ -x "$BIN" ]] || { echo "missing $BIN (build it first)"; exit 1; }

cleanup() {
  docker rm -f $(docker ps -aq --filter "name=^${P}-") >/dev/null 2>&1 || true
  for net in inet lan-a lan-b; do docker network rm "$P-$net" >/dev/null 2>&1 || true; done
}
trap cleanup EXIT
cleanup

# The lab networks have no internet access, so bake the tools into an image first.
printf 'FROM alpine:3.20\nRUN apk add --no-cache iptables iproute2\n' \
  | docker build -q -t "$IMAGE" - >/dev/null

docker network create --internal --subnet 172.30.0.0/24 "$P-inet" >/dev/null
docker network create --internal --subnet 10.10.1.0/24 "$P-lan-a" >/dev/null
docker network create --internal --subnet 10.10.2.0/24 "$P-lan-b" >/dev/null

DHT_ENV=(-e BEEMR_ISOLATED=1 -e BEEMR_DHT_PORT=6881
         -e BEEMR_DHT_BOOTSTRAP=172.30.0.3:6881,172.30.0.4:6881)
CLIENT_ENV=(-e BEEMR_ISOLATED=1 -e BEEMR_DHT_BOOTSTRAP=172.30.0.3:6881,172.30.0.4:6881)

run() {  # name network ip [extra docker args...] -- command...
  local name="$1" net="$2" ip="$3"; shift 3
  docker run -d --name "$P-$name" --network "$P-$net" --ip "$ip" \
    -v "$BIN:/usr/local/bin/beemr:ro" "$@" >/dev/null
}

echo "== starting the simulated internet"
run dht-1 inet 172.30.0.3 "${DHT_ENV[@]}" -e BEEMR_HOME=/data "$IMAGE" beemr dht-node
run dht-2 inet 172.30.0.4 "${DHT_ENV[@]}" -e BEEMR_HOME=/data "$IMAGE" beemr dht-node
if [[ $WITH_RELAY == 1 ]]; then
  run relay inet 172.30.0.5 "${DHT_ENV[@]}" -e BEEMR_HOME=/data "$IMAGE" beemr relay run
fi

make_router() {  # name lan-net lan-ip inet-ip
  local name="$1"
  docker run -d --name "$P-$name" --network "$P-$2" --ip "$3" --cap-add NET_ADMIN \
    --sysctl net.ipv4.ip_forward=1 "$IMAGE" sleep infinity >/dev/null
  docker network connect --ip "$4" "$P-inet" "$P-$name"
  local random=""
  [[ $MODE == symmetric ]] && random="--random-fully"
  docker exec "$P-$name" sh -c "
    IF=\$(ip -o -4 addr show | awk '/$4/ {print \$2}')
    iptables -t nat -A POSTROUTING -o \$IF -j MASQUERADE $random
    # Like a home router: drop unsolicited inbound traffic from the internet.
    iptables -A FORWARD -i \$IF -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT
    iptables -A FORWARD -i \$IF -j DROP"
}
echo "== starting NAT routers ($MODE)"
make_router nat-a lan-a 10.10.1.2 172.30.0.10
make_router nat-b lan-b 10.10.2.2 172.30.0.20

make_host() {  # name lan-net ip gateway
  run "$1" "$2" "$3" --cap-add NET_ADMIN "${CLIENT_ENV[@]}" -e BEEMR_HOME=/data \
    "$IMAGE" sleep infinity
  docker exec "$P-$1" ip route replace default via "$4"
}
make_host host-a lan-a 10.10.1.10 10.10.1.2
make_host host-b lan-b 10.10.2.10 10.10.2.2
docker exec "$P-host-a" beemr name "Alice (lan-a)" >/dev/null
docker exec "$P-host-b" beemr name "Bob (lan-b)" >/dev/null
sleep 3

echo "== host-a shares a 3 MB file"
docker exec "$P-host-a" sh -c "head -c 3000000 /dev/urandom > /tmp/file.bin && md5sum /tmp/file.bin" | awk '{print "   sent md5:     " $1}'
LOG="BEEMR_LOG=libp2p_dcutr=debug,beemr=debug,libp2p_quic=debug,libp2p_swarm=debug"
docker exec -d "$P-host-a" sh -c "$LOG beemr share /tmp/file.bin > /tmp/ticket 2> /tmp/share.log"
for _ in $(seq 1 40); do
  docker exec "$P-host-a" grep -q "beemr get" /tmp/ticket 2>/dev/null && break; sleep 0.5
done
TICKET=$(docker exec "$P-host-a" awk '{print $3}' /tmp/ticket)
sleep 10   # reservations + DHT record

echo "== host-b downloads it"
set +e
START=$(date +%s)
docker exec "$P-host-b" sh -c "mkdir -p /tmp/dl && $LOG beemr get $TICKET -o /tmp/dl 2> /tmp/get.log; s=\$?; grep -v 'DEBUG\\| INFO\\|WARN' /tmp/get.log; exit \$s" 2>&1 | sed 's/^/   /'
STATUS=${PIPESTATUS[0]}
echo "== hole punching diagnostics"
for h in host-a:/tmp/share.log host-b:/tmp/get.log; do
  docker exec "$P-${h%%:*}" sh -c "sed 's/\x1b\[[0-9;]*m//g' ${h#*:} | grep -iE 'hole|dcutr|requesting relay|accepted|172.30.0.(10|20)' | grep -viE 'established_connection.*172.30.0.5|identify' | cut -c 12-600 | head -40" \
    | sed "s/^/   ${h%%:*}: /"
done
echo "   took $(( $(date +%s) - START ))s"
docker exec "$P-host-b" md5sum /tmp/dl/file.bin 2>/dev/null | awk '{print "   received md5: " $1}'
set -e

if [[ $STATUS == 0 ]]; then
  echo "== messaging: host-b runs the background service, host-a sends a message"
  docker exec -d "$P-host-b" sh -c "beemr daemon run 2> /tmp/daemon.log"
  BOB=$(docker exec "$P-host-b" beemr id 2>/dev/null)
  sleep 12
  docker exec "$P-host-a" beemr msg "$BOB" "hello across two NATs" 2>&1 | sed 's/^/   /'
  docker exec "$P-host-b" beemr inbox 2>/dev/null | sed 's/^/   /' | head -6
fi
echo "== done (mode=$MODE relay=$WITH_RELAY, download exit status $STATUS)"
exit "$STATUS"
