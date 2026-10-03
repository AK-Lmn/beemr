#!/usr/bin/env bash
# NAT lab: tests beemr across real Linux NATs, entirely inside Docker.
#
#   lan-a (10.10.1.0/24)              "internet" (172.30.0.0/24)              lan-b (10.10.2.0/24)
#   host-a ── nat-a (MASQUERADE) ──┬── relays, dht-1, dht-2, host-c ──┬── nat-b (MASQUERADE) ── host-b
#
# host-a shares a file; host-b downloads it.
#
# Usage: tests/natlab/run.sh <scenario> [--no-relay]
#   cone       both NATs keep one external port per socket  → hole punching
#   symmetric  both NATs pick a random port per destination → a dedicated relay carries the data
#   mixed      host-a cone, host-b symmetric                → port prediction
#   peerrelay  both symmetric, no dedicated relay; host-c (reachable) is sharing
#              something else and relays for them           → relayed through a user's beemr
#   --no-relay no relay at all: transfers must fail with a clear explanation
#
# RELAYS=1 starts a single dedicated relay instead of two.
#
# Needs a static Linux build of beemr; set BIN to its path (default:
# target/x86_64-unknown-linux-musl/release/beemr). CI runs this on every push.
set -euo pipefail

MODE="${1:-cone}"
WITH_RELAY=1
[[ "${2:-}" == "--no-relay" || $MODE == peerrelay ]] && WITH_RELAY=0
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
BIN="${BIN:-$ROOT/target/x86_64-unknown-linux-musl/release/beemr}"
IMAGE="bplab-node"
P="bplab"   # resource prefix
[[ -x "$BIN" ]] || { echo "missing $BIN (build it first)"; exit 1; }

case $MODE in
  cone)      RANDOM_A="";              RANDOM_B="" ;;
  symmetric) RANDOM_A="--random-fully"; RANDOM_B="--random-fully" ;;
  mixed)     RANDOM_A="";              RANDOM_B="--random-fully" ;;
  peerrelay) RANDOM_A="--random-fully"; RANDOM_B="--random-fully" ;;
  *) echo "unknown scenario $MODE"; exit 2 ;;
esac

cleanup() {
  docker rm -f $(docker ps -aq --filter "name=^${P}-") >/dev/null 2>&1 || true
  for net in inet lan-a lan-b; do docker network rm "$P-$net" >/dev/null 2>&1 || true; done
}
[[ -n "${KEEP:-}" ]] || trap cleanup EXIT   # KEEP=1 leaves the lab running for inspection
cleanup

# The lab networks have no internet access, so bake the tools into an image first.
printf 'FROM alpine:3.20\nRUN apk add --no-cache iptables iproute2\n' \
  | docker build -q -t "$IMAGE" - >/dev/null

# Newer Docker Desktop drops routed traffic on internal networks; set
# LAB_INTERNAL=0 there (the lab then has a route out, which beemr ignores).
INTERNAL=--internal
[[ "${LAB_INTERNAL:-1}" == 0 ]] && INTERNAL=
docker network create $INTERNAL --subnet 172.30.0.0/24 "$P-inet" >/dev/null
docker network create $INTERNAL --subnet 10.10.1.0/24 "$P-lan-a" >/dev/null
docker network create $INTERNAL --subnet 10.10.2.0/24 "$P-lan-b" >/dev/null

DHT_ENV=(-e BEEMR_ISOLATED=1 -e BEEMR_DHT_PORT=6881
         -e BEEMR_DHT_BOOTSTRAP=172.30.0.3:6881,172.30.0.4:6881)
CLIENT_ENV=(-e BEEMR_ISOLATED=1 -e BEEMR_DHT_BOOTSTRAP=172.30.0.3:6881,172.30.0.4:6881)

run() {  # name network ip [extra docker args...] -- command...
  local name="$1" net="$2" ip="$3"; shift 3
  docker run -d --name "$P-$name" --network "$P-$net" --ip "$ip" \
    -v "$BIN:/usr/local/bin/beemr:ro" "$@" >/dev/null
}

echo "== starting the simulated internet ($MODE)"
run dht-1 inet 172.30.0.3 "${DHT_ENV[@]}" -e BEEMR_HOME=/data "$IMAGE" beemr dht-node
run dht-2 inet 172.30.0.4 "${DHT_ENV[@]}" -e BEEMR_HOME=/data "$IMAGE" beemr dht-node
if [[ $WITH_RELAY == 1 ]]; then
  # Two relays, so each device has two peers reporting its external address
  # (that's how beemr tells cone from symmetric NAT).
  run relay  inet 172.30.0.5 "${DHT_ENV[@]}" -e BEEMR_HOME=/data "$IMAGE" beemr relay run
  [[ "${RELAYS:-2}" == 1 ]] || run relay2 inet 172.30.0.6 "${DHT_ENV[@]}" -e BEEMR_HOME=/data "$IMAGE" beemr relay run
fi
if [[ $MODE == peerrelay ]]; then
  # Another beemr user with a public address, sharing something unrelated.
  run host-c inet 172.30.0.7 "${CLIENT_ENV[@]}" -e BEEMR_HOME=/data -e BEEMR_ASSUME_REACHABLE=1 \
    "$IMAGE" sh -c "beemr name 'Carol (public)' >/dev/null; echo hi > /tmp/x; beemr share /tmp/x -n 0 > /dev/null 2> /tmp/share.log; sleep infinity"
fi

make_router() {  # name lan-net lan-ip inet-ip masquerade-options
  local name="$1"
  docker run -d --name "$P-$name" --network "$P-$2" --ip "$3" --cap-add NET_ADMIN \
    --sysctl net.ipv4.ip_forward=1 "$IMAGE" sleep infinity >/dev/null
  docker network connect --ip "$4" "$P-inet" "$P-$name"
  docker exec "$P-$name" sh -c "
    IF=\$(ip -o -4 addr show | awk '/$4/ {print \$2}')
    iptables -t nat -A POSTROUTING -o \$IF -j MASQUERADE $5
    # Like a home router: drop unsolicited inbound traffic from the internet,
    # both to the LAN and to the router itself. (Accepting it locally would
    # leave conntrack entries that force the NAT onto other ports.)
    for chain in FORWARD INPUT; do
      iptables -A \$chain -i \$IF -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT
      iptables -A \$chain -i \$IF -j DROP
    done"
}
echo "== starting NAT routers (a: ${RANDOM_A:-cone}, b: ${RANDOM_B:-cone})"
make_router nat-a lan-a 10.10.1.2 172.30.0.10 "$RANDOM_A"
make_router nat-b lan-b 10.10.2.2 172.30.0.20 "$RANDOM_B"

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
LOG="BEEMR_LOG=libp2p_dcutr=debug,beemr=debug"
docker exec -d "$P-host-a" sh -c "$LOG beemr share /tmp/file.bin > /tmp/ticket 2> /tmp/share.log"
for _ in $(seq 1 40); do
  docker exec "$P-host-a" grep -q "beemr get" /tmp/ticket 2>/dev/null && break; sleep 0.5
done
TICKET=$(docker exec "$P-host-a" awk '{print $3}' /tmp/ticket)
sleep 12   # relay reservations, NAT classification and the DHT record

echo "== host-b downloads it"
set +e
START=$(date +%s)
docker exec "$P-host-b" sh -c "mkdir -p /tmp/dl && $LOG beemr get $TICKET -o /tmp/dl 2> /tmp/get.log; s=\$?; grep -v 'DEBUG\\| INFO\\|WARN' /tmp/get.log; exit \$s" 2>&1 | sed 's/^/   /'
STATUS=${PIPESTATUS[0]}
echo "   took $(( $(date +%s) - START ))s"
docker exec "$P-host-b" md5sum /tmp/dl/file.bin 2>/dev/null | awk '{print "   received md5: " $1}'
echo "== sender's view"
docker exec "$P-host-a" sh -c "sed 's/\x1b\[[0-9;]*m//g' /tmp/share.log | grep -vE 'DEBUG|INFO|WARN' | grep -E 'Sending to|How:|received everything|Refused|failed'" | sed 's/^/   /'
echo "== diagnostics"
for h in host-a:/tmp/share.log host-b:/tmp/get.log; do
  docker exec "$P-${h%%:*}" sh -c "sed 's/\x1b\[[0-9;]*m//g' ${h#*:} | grep -iE 'hole punching finished|port prediction|requesting relay|connection established.*(10\.10|172\.30\.0\.(10|20))' | cut -c 12-400 | head -12" \
    | sed "s/^/   ${h%%:*}: /"
done
set -e

echo "== done (scenario=$MODE relay=$WITH_RELAY, download exit status $STATUS)"
exit "$STATUS"
