#!/usr/bin/env bash
# Runs every command shown in README.md and `beemr help` against a built
# binary, as several simulated devices on one machine, and reports PASS/FAIL.
#
#   tests/docs-smoke.sh [path/to/beemr]
#
# Devices are isolated from public networks and find each other through a
# private Mainline DHT started by this script. Set SMOKE_PUBLIC=1 to also run
# `beemr doctor` against the real internet, and SMOKE_SERVICE=1 to install
# and remove the background service with the OS service manager.
set -uo pipefail

BIN="${1:-$(cd "$(dirname "$0")/.." && pwd)/target/release/beemr}"
[[ -x "$BIN" ]] || { echo "binary not found: $BIN (run cargo build --release)"; exit 2; }
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"   # tests change directory
W="$(mktemp -d "${TMPDIR:-/tmp}/beemr-smoke.XXXXXX")"
DHT_PORT_A=$((40000 + RANDOM % 10000)); DHT_PORT_B=$((DHT_PORT_A + 1))
BOOT="127.0.0.1:$DHT_PORT_A,127.0.0.1:$DHT_PORT_B"
PASS=0; FAIL=0; PIDS=()

# stop <pid> [signal]: stop a backgrounded `bp` call. The pid belongs to the
# subshell running the function, so signal its children (beemr) first.
stop() {
  pkill "-${2:-TERM}" -P "$1" 2>/dev/null
  kill "-${2:-TERM}" "$1" 2>/dev/null
}

cleanup() {
  for pid in "${PIDS[@]:-}"; do stop "$pid"; done
  wait 2>/dev/null
  rm -rf "$W"
}
trap cleanup EXIT

pass() { PASS=$((PASS + 1)); printf '  \033[32mPASS\033[0m %s\n' "$1"; }
fail() { FAIL=$((FAIL + 1)); printf '  \033[31mFAIL\033[0m %s\n' "$1"; [[ -n "${2:-}" ]] && sed 's/^/         /' <<<"$2" | head -15; }
section() { printf '\n\033[1m%s\033[0m\n' "$1"; }

# bp <device> <args...>: run beemr as a device (with a timeout).
bp() {
  local dev="$1"; shift
  BEEMR_HOME="$W/$dev/config" BEEMR_ISOLATED=1 BEEMR_DHT_BOOTSTRAP="$BOOT" \
    perl -e 'alarm shift; exec @ARGV' "${TIMEOUT:-120}" "$BIN" "$@"
}

# expect <description> <grep pattern> <command...>: command succeeds and output matches.
expect() {
  local desc="$1" pattern="$2"; shift 2
  local out; out="$("$@" 2>&1)"; local status=$?
  if [[ $status -eq 0 ]] && grep -qE -- "$pattern" <<<"$out"; then pass "$desc"; else fail "$desc (exit $status)" "$out"; fi
}

# expect_fail <description> <grep pattern> <command...>: command fails and output matches.
expect_fail() {
  local desc="$1" pattern="$2"; shift 2
  local out; out="$("$@" 2>&1)"; local status=$?
  if [[ $status -ne 0 ]] && grep -qE -- "$pattern" <<<"$out"; then pass "$desc"; else fail "$desc (exit $status)" "$out"; fi
}

# share <device> <path> [options...]: start sharing in the background, set TICKET and SHARE_PID.
share() {
  local dev="$1"; shift
  local log="$W/$dev/share-$RANDOM"
  bp "$dev" share "$@" > "$log.out" 2> "$log.err" &
  SHARE_PID=$!; SHARE_LOG="$log"; PIDS+=("$SHARE_PID")
  TICKET=""
  for _ in $(seq 1 60); do
    TICKET="$(awk '/beemr get/ {print $3}' "$log.out" 2>/dev/null)"
    [[ -n "$TICKET" ]] && break; sleep 0.25
  done
  [[ -n "$TICKET" ]] || fail "share $* printed a ticket" "$(cat "$log.err")"
}

# wait_exit <pid> <seconds>: wait for a process to exit; returns 1 on timeout.
wait_exit() {
  for _ in $(seq 1 $(($2 * 4))); do kill -0 "$1" 2>/dev/null || { wait "$1" 2>/dev/null; return 0; }; sleep 0.25; done
  return 1
}

eventually() {  # eventually <seconds> <command...>
  local deadline=$((SECONDS + $1)); shift
  while (( SECONDS < deadline )); do "$@" >/dev/null 2>&1 && return 0; sleep 1; done
  return 1
}

echo "beemr smoke test: $("$BIN" --version)   (work dir $W)"
mkdir -p "$W"/{alice,sara,eve,relay,dht-a,dht-b}

section "Private DHT for discovery"
BEEMR_HOME="$W/dht-a" BEEMR_ISOLATED=1 BEEMR_DHT_PORT=$DHT_PORT_A BEEMR_DHT_BOOTSTRAP=127.0.0.1:$DHT_PORT_B \
  "$BIN" dht-node 2>/dev/null & PIDS+=($!)
BEEMR_HOME="$W/dht-b" BEEMR_ISOLATED=1 BEEMR_DHT_PORT=$DHT_PORT_B BEEMR_DHT_BOOTSTRAP=127.0.0.1:$DHT_PORT_A \
  "$BIN" dht-node 2>/dev/null & PIDS+=($!)
sleep 1; pass "two DHT nodes started on ports $DHT_PORT_A, $DHT_PORT_B"

section "Basics: help, version, errors"
expect "beemr (no arguments) prints usage" "Usage|Files:" "$BIN"
expect "beemr help lists every command" "msg <contact-or-id>.*" "$BIN" help
expect "beemr --help" "beemr share" "$BIN" --help
expect "beemr --version" "^beemr [0-9]+\.[0-9]+\.[0-9]+" "$BIN" --version
expect_fail "unknown command is rejected helpfully" "unknown command 'frobnicate'" "$BIN" frobnicate

section "This device: setup, id, name"
expect "beemr setup --name ... --no-service" "This device is \"Alice's laptop\"" bp alice setup --name "Alice's laptop" --no-service
ALICE="$(bp alice id 2>/dev/null)"
[[ ${#ALICE} -eq 52 ]] && pass "beemr id prints a 52-character device ID" || fail "beemr id" "$ALICE"
[[ "$(bp alice id 2>/dev/null)" == "$ALICE" ]] && pass "the device ID is stable across runs" || fail "stable ID"
expect "beemr name shows the name" "^Alice's laptop$" bp alice name
expect "beemr name \"Osman's Mac\" renames the device" "now called \"Osman's Mac\"" bp alice name "Osman's Mac"
expect "the new name is saved" "^Osman's Mac$" bp alice name
[[ "$(bp alice id 2>/dev/null)" == "$ALICE" ]] && pass "renaming keeps the same device ID" || fail "ID changed after rename"
bp alice name "Alice" >/dev/null 2>&1
expect "setup for Sara" "This device is \"Sara\"" bp sara setup --name "Sara" --no-service
expect "setup for Eve" "This device is \"Eve\"" bp eve setup --name "Eve" --no-service
SARA="$(bp sara id 2>/dev/null)"; EVE="$(bp eve id 2>/dev/null)"

section "Contacts"
expect "beemr contact add Sara <id>" "Saved Sara" bp alice contact add Sara "$SARA"
expect "beemr contact add with a multi-word name" "Saved Eve Adams" bp alice contact add Eve Adams "$EVE"
expect "beemr contact list" "^Sara	$SARA" bp alice contact list
expect "beemr contact (no subcommand) lists too" "Eve Adams" bp alice contact
expect "beemr contact remove" "Removed Eve Adams" bp alice contact remove Eve Adams
expect_fail "contact add rejects an invalid ID" "not a device ID" bp alice contact add Bob notanid
expect_fail "contact remove of a missing contact fails" "no contact named" bp alice contact remove Nobody
expect "Sara saves Alice as a contact" "Saved Alice" bp sara contact add Alice "$ALICE"

section "Files: share and get"
mkdir -p "$W/alice/files/photos/2024/summer" "$W/alice/files/photos/empty" "$W/sara/dl" "$W/sara/Downloads"
head -c 1500000 /dev/urandom > "$W/alice/files/report.pdf"
head -c 400000 /dev/urandom > "$W/alice/files/photos/2024/summer/beach.jpg"
echo "caption" > "$W/alice/files/photos/2024/notes.txt"
: > "$W/alice/files/photos/empty.txt"
echo "contract" > "$W/alice/files/contract.pdf"

share alice "$W/alice/files/report.pdf"
expect "share report.pdf → get <ticket> (into the current folder)" "Saved to" \
  bash -c "cd '$W/sara/dl' && BEEMR_HOME='$W/sara/config' BEEMR_ISOLATED=1 BEEMR_DHT_BOOTSTRAP='$BOOT' '$BIN' get '$TICKET'"
cmp -s "$W/alice/files/report.pdf" "$W/sara/dl/report.pdf" && pass "report.pdf arrived byte-identical" || fail "report.pdf corrupted or missing"
wait_exit "$SHARE_PID" 20 && pass "the sharer stops after one download (default)" || fail "sharer kept running after 1 download"
grep -q "download limit reached" "$SHARE_LOG.err" && pass "the sharer says the limit was reached" || fail "limit message" "$(cat "$SHARE_LOG.err")"
grep -q "Sending to Sara" "$SHARE_LOG.err" && pass "the sharer shows Sara by her contact name" || fail "sharer shows contact name" "$(cat "$SHARE_LOG.err")"
expect_fail "the same ticket can't be used again" "couldn't|reach|find" bp sara get "$TICKET" -o "$W/sara/dl"

share alice "$W/alice/files/photos" -n 3 -e 2h
grep -q "Downloads allowed: 3" "$SHARE_LOG.err" && grep -q "Expires:           in 2h" "$SHARE_LOG.err" \
  && pass "share photos/ -n 3 -e 2h shows the policy" || fail "policy display" "$(cat "$SHARE_LOG.err")"
for i in 1 2 3; do expect "download $i of 3" "Saved to" bp sara get "$TICKET" -o "$W/sara/Downloads"; done
diff -r "$W/alice/files/photos" "$W/sara/Downloads/photos" >/dev/null && pass "the folder tree arrived identical (incl. empty files and folders)" || fail "folder differs"
[[ -d "$W/sara/Downloads/photos (1)" && -d "$W/sara/Downloads/photos (2)" ]] && pass "repeat downloads get numbered names instead of overwriting" || fail "numbered copies" "$(ls "$W/sara/Downloads")"
wait_exit "$SHARE_PID" 20 && pass "the sharer stops after 3 downloads" || fail "sharer didn't stop after 3"
expect_fail "a 4th download is refused" "couldn't|reach|limit" bp sara get "$TICKET" -o "$W/sara/Downloads"
[[ -z "$(ls -A "$W/sara/Downloads" | grep beemr-partial)" ]] && pass "no partial downloads left behind" || fail "partial files left"

share alice "$W/alice/files/contract.pdf" --to sara
grep -q "Who can download:  only Sara" "$SHARE_LOG.err" && pass "share --to sara shows who can download" || fail "--to display" "$(cat "$SHARE_LOG.err")"
expect_fail "Eve is refused (--to sara)" "different device" bp eve get "$TICKET" -o "$W/sara/dl"
grep -q "Refused" "$SHARE_LOG.err" && pass "the sharer reports the refused device" || fail "refusal not reported"
expect "Sara can download (--to sara)" "Saved to" bp sara get "$TICKET" -o "$W/sara/dl"
wait_exit "$SHARE_PID" 20 && pass "the sharer stops after Sara's download" || fail "sharer kept running"

share alice "$W/alice/files/report.pdf" -e 2s
sleep 3
wait_exit "$SHARE_PID" 10 && grep -q "expired" "$SHARE_LOG.err" && pass "share -e 2s expires and stops by itself" || fail "expiry" "$(cat "$SHARE_LOG.err")"
expect_fail "an expired share is refused" "couldn't|reach|expired" bp sara get "$TICKET" -o "$W/sara/dl"

PORT=$((30000 + RANDOM % 5000))
share alice "$W/alice/files/report.pdf" -p "$PORT" --no-upnp -n 0
python3 - "$TICKET" "$PORT" <<'EOF' && pass "share -p $PORT listens on the requested port" || fail "share -p"
import base64, sys
t = sys.argv[1]; b = base64.urlsafe_b64decode(t + "=" * (-len(t) % 4))
sys.exit(0 if int.from_bytes(b[49:51], "big") == int(sys.argv[2]) else 1)
EOF
expect "share -n 0 allows repeated downloads (1)" "Saved to" bp sara get "$TICKET" -o "$W/sara/Downloads"
expect "share -n 0 allows repeated downloads (2)" "Saved to" bp sara get "$TICKET" -o "$W/sara/Downloads"
stop "$SHARE_PID" INT; wait_exit "$SHARE_PID" 10 && grep -q "Stopped sharing" "$SHARE_LOG.err" \
  && pass "Ctrl+C stops an unlimited share" || fail "Ctrl+C" "$(tail -3 "$SHARE_LOG.err")"

expect_fail "get rejects an invalid ticket" "ticket" bp sara get not-a-real-ticket
expect_fail "get -o rejects a missing folder" "not a folder" bp sara get "$TICKET" -o "$W/does-not-exist"
expect_fail "share rejects a missing file" "No such file|not found|cannot find" bp alice share "$W/nope.txt"
expect_fail "share rejects unknown options" "unknown option --bogus" bp alice share "$W/alice/files/report.pdf" --bogus

section "Messages: msg, inbox, reply, queue"
expect "inbox is empty at first" "empty" bp sara inbox
expect_fail "daemon status (not running, exit code 3)" "not running" bp sara daemon status
bp sara daemon run 2> "$W/sara/daemon.log" & SARA_DAEMON=$!; PIDS+=("$SARA_DAEMON")
eventually 20 bp sara daemon status && expect "daemon status (running)" "is running" bp sara daemon status
expect_fail "a second background service refuses to start" "already running" bp sara daemon run
if eventually 90 bash -c "BEEMR_HOME='$W/alice/config' BEEMR_ISOLATED=1 BEEMR_DHT_BOOTSTRAP='$BOOT' '$BIN' msg sara 'are you free tonight?' 2>&1 | grep -q Delivered"; then
  pass "beemr msg sara \"are you free tonight?\" → Delivered"
else
  fail "msg was never delivered" "$(tail -5 "$W/sara/daemon.log")"
fi
expect "Sara's inbox shows the message from Alice by contact name" "Alice · .*" bp sara inbox
expect "the message text is shown" "are you free tonight\?" bp sara inbox
expect "messages are marked read after viewing" "0 new" bp sara inbox
expect "inbox --all" "are you free tonight\?" bp sara inbox --all
expect "Eve messages Sara (unknown sender)" "Delivered" bp eve msg "$SARA" "hi, I'm Eve"
expect "unknown senders are marked as not in contacts" "\"Eve\" \(not in contacts" bp sara inbox
expect "inbox suggests saving unknown senders" "contact add <name> $EVE" bp sara inbox

bp alice daemon run 2> "$W/alice/daemon.log" & ALICE_DAEMON=$!; PIDS+=("$ALICE_DAEMON")
sleep 5
if eventually 90 bash -c "BEEMR_HOME='$W/sara/config' BEEMR_ISOLATED=1 BEEMR_DHT_BOOTSTRAP='$BOOT' '$BIN' reply 2 'yes, 8pm' 2>&1 | grep -q Delivered"; then
  pass "beemr reply 2 \"yes, 8pm\" (to Alice) → Delivered"
else
  fail "reply was never delivered"
fi
expect "Alice's inbox shows Sara's reply" "yes, 8pm" bp alice inbox
expect_fail "reply to a non-existent message number fails" "no message number 99" bp sara reply 99 "hello"
expect_fail "msg to an unknown contact fails helpfully" "neither a saved contact nor a device ID" bp alice msg bob "hi"
expect_fail "empty messages are rejected" "empty" bp alice msg sara "   "

stop "$SARA_DAEMON"; wait_exit "$SARA_DAEMON" 10
eventually 10 bash -c "! BEEMR_HOME='$W/sara/config' '$BIN' daemon status" \
  && pass "Sara's background service stopped" || fail "Sara's service is still running"
TIMEOUT=90 expect "msg to an offline device is queued" "Queued" bp alice msg sara "message while you were away"
expect "daemon status shows the queued message" "1 message\(s\) waiting" bp alice daemon status
bp sara daemon run 2> "$W/sara/daemon2.log" & PIDS+=($!)
if eventually 180 bash -c "BEEMR_HOME='$W/sara/config' '$BIN' inbox 2>/dev/null | grep -q 'message while you were away'"; then
  pass "the queued message is delivered automatically when Sara comes online"
else
  fail "queued message never arrived" "$(tail -5 "$W/alice/daemon.log")"
fi

section "Relays"
RELAY_PORT=$((35000 + RANDOM % 4000))
bp relay relay run --private --port "$RELAY_PORT" 2> "$W/relay/relay.log" & PIDS+=($!)
eventually 30 grep -q "relay use" "$W/relay/relay.log"
RELAY_ADDR="$(grep -o 'relay use /ip4/[^ ]*' "$W/relay/relay.log" | head -1 | cut -d' ' -f3)"
[[ -n "$RELAY_ADDR" ]] && pass "beemr relay --private prints addresses to use" || fail "relay addresses" "$(cat "$W/relay/relay.log")"
grep -q "Not listed publicly" "$W/relay/relay.log" && pass "--private keeps the relay out of the public list" || fail "--private message"
expect "beemr relay use <address>" "Saved" bp alice relay use "$RELAY_ADDR"
expect "beemr relay list" "$RELAY_ADDR" bp alice relay list
expect_fail "relay use rejects an address without /p2p/<id>" "must end in /p2p" bp alice relay use /ip4/1.2.3.4/tcp/4545
expect "beemr relay remove <address>" "Removed" bp alice relay remove "$RELAY_ADDR"
expect "relay list is empty again" "No saved relays" bp alice relay list

if [[ "${SMOKE_SERVICE:-}" == 1 ]]; then
  section "Background service (OS service manager)"
  expect "beemr daemon install" "Background service running" env BEEMR_HOME="$W/svc/config" "$BIN" daemon install
  eventually 20 env BEEMR_HOME="$W/svc/config" "$BIN" daemon status
  expect "the service is running after install" "is running" env BEEMR_HOME="$W/svc/config" "$BIN" daemon status
  expect "beemr daemon uninstall" "stopped and removed" env BEEMR_HOME="$W/svc/config" "$BIN" daemon uninstall
  sleep 2
  expect_fail "the service is stopped after uninstall" "not running" env BEEMR_HOME="$W/svc/config" "$BIN" daemon status
fi

if [[ "${SMOKE_PUBLIC:-}" == 1 ]]; then
  section "Public internet"
  TIMEOUT=90 expect "beemr doctor (real network)" "Public address|Hole punching" \
    env BEEMR_HOME="$W/doctor/config" perl -e 'alarm 90; exec @ARGV' "$BIN" doctor
fi

printf '\n\033[1m%d passed, %d failed\033[0m\n' "$PASS" "$FAIL"
[[ $FAIL -eq 0 ]]
