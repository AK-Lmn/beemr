# The beemr protocol, version 2

This document specifies how beemr devices find each other, connect
through NATs, authenticate, and transfer files. It is complete
enough to write an interoperable implementation without reading the
reference code.

The key words MUST, MUST NOT, SHOULD and MAY are used as in RFC 2119.

## 1. Goals

- **No servers of our own.** Devices connect directly. When they can't,
  public peer-to-peer infrastructure or other beemr devices help, and
  never see plaintext.
- **No configuration.** Devices behind ordinary home or mobile NATs reach
  each other without router setup.
- **End-to-end security.** All traffic is encrypted and authenticated between
  the two devices, including traffic through relays.
- **Device identities.** Every device has a long-term key, and shares can be
  restricted to specific devices.

## 2. Building blocks

| Layer | Technology |
|---|---|
| Transport | libp2p over QUIC v1 and TCP, IPv4 and IPv6 |
| Security | libp2p Noise XX (X25519, ChaCha20-Poly1305) on TCP, TLS 1.3 on QUIC |
| Multiplexing | Yamux (TCP), native streams (QUIC) |
| NAT traversal | UPnP-IGD port mapping, AutoNAT v2, Circuit Relay v2, DCUtR hole punching |
| Discovery | BEP 44 signed mutable items on the BitTorrent Mainline DHT |
| Relay discovery | Kademlia on the public IPFS DHT (for hole-punch coordinators); BEP 5 announces on Mainline (for beemr relays) |

All integers are big-endian. `HMAC` is HMAC-SHA256. `||` is concatenation.
Labels are ASCII strings without a terminator.

## 3. Identities

### 3.1 Device identity

Each device generates an Ed25519 key pair on first use and stores the 32-byte
seed privately. The **device ID** is the public key, encoded as RFC 4648 base32,
lowercase, without padding (52 characters). Implementations SHOULD accept IDs
case-insensitively and ignore separators.

Each device also has a **name** chosen by its user, 1–64 characters with no
control characters. Names are self-asserted and MUST NOT be treated as
authenticated. Implementations SHOULD prefer the user's own contact name for
a device, and otherwise mark a self-chosen name as unverified.

### 3.2 Network identity

The libp2p peer ID used on the network is separate from the device identity.
It SHOULD be a fresh Ed25519 key for each process, so several beemr
processes for one device can run at once. Relays SHOULD use a stable key
derived from the device seed (`HMAC(seed, "beemr/derive/relay-key")`), so
their addresses stay valid across restarts.

The device identity is bound to each connection by the `Hello` signature (§6).

## 4. Discovery records

While sharing, a device publishes where it can be reached as a BEP 44
**mutable item** on the Mainline DHT. The item is signed with the device key,
so its public key *is* the device ID. The `seq` field is the current Unix time
in microseconds. The salt is:

```
salt = HMAC(share_secret, "beemr/share-record")[0..16]
```

Because the salt is derived from the share's secret, the record can't be found
without the ticket.

The value (at most 1000 bytes) is:

```
value   = 0x01 || peer_len (u8) || peer_id || name_len (u8) || name
          || count (u8) || entry*
entry   = flag (u8) || addr_len (u16) || multiaddr
flag    = 0x00 direct or limited-relay address | 0x01 full-relay circuit
```

Publishers SHOULD list, in this order: full-relay circuits, addresses
confirmed reachable (UPnP, AutoNAT), public and LAN listen addresses, and
limited-relay circuits. They drop entries that don't fit in 1000 bytes. Every
relayed address MUST end in `/p2p-circuit/p2p/<publisher peer id>`.
Publishers MUST republish when their addresses change, and at least every 15
minutes.

## 5. Connecting

### 5.1 Tickets

A share is identified by a ticket:

```
ticket = base64url_nopad( 0x02 || device_id (32) || secret (16) || port (u16) || lan* )
lan    = 0x04 || ipv4 (4) | 0x06 || ipv6 (16)
```

`lan` holds up to 8 local-network addresses of the sharer. Together with
`port`, which is used for both TCP and QUIC, they let devices on the same
network connect without any internet access.

### 5.2 The connection ladder

To reach a device, the initiator does all of the following at once:

1. Dials every ticket LAN address over QUIC and TCP, with an unknown peer ID.
2. Resolves the relevant DHT record and dials all its addresses, including
   relay circuits. It re-resolves every ~10 s while not yet connected, because
   relays come and go.
3. When a connection through a relay is established, both sides run **DCUtR**.
   The side that accepted the relayed connection initiates, and both dial each
   other's observed addresses at the same moment.

It then keeps the best connection:

- A **direct** connection is always preferred. All relayed connections to
  that peer SHOULD then be closed, so streams don't pick a limited one.
- After about 16 s without a successful hole punch,
  the initiator uses a connection through a **full relay** (§5.3), if one
  exists.

If nothing works, implementations SHOULD explain which step failed and how
to fix it.

### 5.3 Relays

A **limited relay** reports a circuit limit below 64 MiB or 10 minutes. Public
IPFS relays are limited (typically 128 KiB and 2 minutes). They are used only
to coordinate hole punching.

A **full relay** allows at least 64 MiB and 10 minutes per circuit, and can
carry transfers. beemr devices run a full relay server when they are
publicly reachable. Their circuits carry Noise- or TLS-encrypted libp2p
traffic, so relays can't read it.

- Sharers SHOULD hold reservations on two limited
  relays (found via the IPFS DHT, preferring QUIC) and up to two full relays.
- Full relays announce themselves with BEP 5 `announce_peer` under the
  infohash `SHA-256("beemr/relays/1")[0..20]`, on the port they listen on
  for both TCP and QUIC. Devices find them with `get_peers` and recognise
  them by the Identify agent version prefix `beemr/`.
- Users MAY configure additional relays by multiaddress.

Relay reservations MUST be made only over plain TCP or QUIC addresses, and
relayed addresses using other transports MUST NOT be published.

## 6. The beemr stream protocol

After a connection is established, the initiator opens a libp2p stream with
protocol ID `/beemr/2`. Messages are framed as:

```
frame = length (u32, 1..=131072) || message
message = type (u8) || body
```

| Type | Name | Body |
|---:|---|---|
| 1 | Hello | device_id (32) · signature (64) · name (UTF-8, rest) |
| 2 | Header | total_bytes (u64) · file_count (u64) · name (UTF-8, rest) |
| 3 | Entry | kind (u8: 0 file, 1 dir) · mode (u32) · size (u64) · path (UTF-8, rest) |
| 4 | Data | file bytes, at most 65536 |
| 5 | End | total_bytes (u64) |
| 6 | Ack | empty |
| 7 | Fail | reason (UTF-8, rest) |
| 8 | Download | proof (32) |
| 9, 10 | *reserved* | used by text messages before 0.3; MUST NOT be reused |

Messages with trailing bytes or unknown types MUST be rejected.

### 6.1 Hello

The initiator sends `Hello` first; the responder verifies it and replies with
its own. Each side signs:

```
"beemr/hello/initiator" || own_peer_id || remote_peer_id     (initiator)
"beemr/hello/responder" || own_peer_id || remote_peer_id     (responder)
```

using the bytes of the libp2p peer IDs of this connection. Verifiers MUST
check the signature against the claimed device ID, using the peer IDs that the
libp2p security handshake authenticated. This binds the long-term device
identity to the encrypted session, so neither a relay nor a man-in-the-middle
can splice or replay identities.

### 6.2 Downloading a share

```
initiator → Hello
responder → Hello
initiator → Download { proof = HMAC(secret, "beemr/download" || initiator_peer || responder_peer) }
responder → Header, (Entry, Data*)*, End        or  Fail(reason)
initiator → Ack                                 or  Fail(reason)
```

The responder MUST verify `proof` in constant time. It then applies its
policy:

- If the share is restricted and the initiator's device ID isn't allowed, it
  sends `Fail("this share is for a different device")`.
- If the share has expired or reached its download limit, it sends `Fail`
  with the reason.

A download limit MUST count transfers in progress as well as completed ones.
A failed transfer frees its slot again. After expiry, a sharer MUST refuse new
transfers but MAY finish those in progress.

The initiator MUST check that the responder's device ID equals the ticket's
device ID.

The entry rules:

- `Header.name` MUST be a single safe path component.
- Each `Entry.path` is `/`-separated, starts with `Header.name`, and lists
  directories before their contents.
- A file's `Data` messages total exactly `Entry.size`.
- `End.total_bytes` MUST equal the sum of file sizes.

### 6.3 Closing

After its final message, a side SHOULD close its write half and wait briefly
for the peer to close theirs before exiting. Otherwise the last message may be
lost in flight.

## 7. Receiver safety requirements

- Every path component MUST be non-empty, MUST NOT be `.` or `..`, and MUST
  NOT contain `/`, `\` or NUL. On Windows it MUST NOT contain `: < > " | ? *`.
- Files MUST be written to a fresh staging directory and moved into place
  only after `End` is verified.
- Existing files MUST NOT be overwritten; choose a free name such as
  `name (1).ext`.
- Receivers MUST NOT create symbolic links. Senders SHOULD skip links and
  special files.
- Only the executable bits of `mode` SHOULD be applied.

## 8. Security considerations

- **Confidentiality.** Every byte between devices is protected by libp2p's
  Noise or TLS 1.3 session, end to end, including through relays. DHT records
  are public: they reveal a device's network addresses and self-chosen name,
  but never content.
- **Authentication.** Peer IDs are authenticated by the libp2p handshake, and
  device IDs by the `Hello` signatures bound to those peer IDs. Device IDs are
  trust-on-first-use; verify one out of band before saving it as a contact.
- **Tickets.** An unrestricted share is only as private as its ticket. Use
  `--to` for anything sensitive. The default download limit of one reduces
  what a leaked ticket can do.
- **Relays.** Relays learn which peers talk, when, and roughly how much, but
  not what. Full relays SHOULD rate-limit reservations and circuits.
- **Denial of service.** Unidentified streams SHOULD time out after about
  20 s, and transfers after about 60 s of inactivity.

## 9. Future work

- Resumable transfers (an offset in `Entry`) and per-file hashes.
- Sending files to a device ID directly, without a ticket.
