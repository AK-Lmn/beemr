<p align="center">
  <img src="docs/beemr-logo.png" alt="beemr" width="560">
</p>

<p align="center"><b>Send files and messages straight to another device. No servers, no accounts, no setup.</b></p>

```console
$ beemr share vacation-photos/
Sharing "vacation-photos" (214 files, 1.3 GB)

Who can download:  anyone with the command below
Downloads allowed: 1
Expires:           never

On the other device, run:

    beemr get AlTRKOzvjlzG5IBDDhhzcwj56mci3evFmCm22FhZ8NVAOe29ocweZCkDP1kmbFeakaMFBMCoARc

Waiting for the other device… (Ctrl+C to stop sharing)

  ✓ Reachable on your local network
  ✓ Hole punching ready (through firewalls, no setup needed)
```

On the other device:

```console
$ beemr get AlTRKOzvjlzG5IBDDhhzcwj56mci3evFmCm22FhZ8NVAOe29ocweZCkDP1kmbFeakaMFBMCoARc
Connecting…
Receiving "vacation-photos" (214 files, 1.3 GB) from Osman
  via direct connection (through the firewall via hole punching)
  Receiving [========================] 100.0%  1.3 GB / 1.3 GB  48.2 MB/s
Saved to ./vacation-photos
```

Messages:

```console
$ beemr msg sara "landed! call you later"
Sending to Sara…
Delivered (direct connection).

$ beemr inbox
Inbox: 1 message, 1 new

●  1  Sara · 2 min ago
      welcome home 🎉

Reply with: beemr reply <number> <message>
```

- **Works across the internet with zero configuration.** beemr connects directly
  when it can, punches through home and mobile NATs when it can't, and falls back
  to relaying through other beemr devices.
- **No servers.** Devices find each other through the BitTorrent DHT and connect
  peer to peer. Nothing is uploaded anywhere.
- **End-to-end encrypted.** Every byte is encrypted between the two devices, even
  when it passes through a relay.
- **Device identities and names.** Each device has a cryptographic ID and a name
  you choose. Share with `--to sara` and only Sara's device can download.
- **Limits.** One download by default, or `-n 5`. Expires after `-e 10m`.
- **Messages and an inbox** that show names, not IDs.
- **One small native binary** (about 9 MB) for macOS, Linux and Windows.

## Install

**macOS and any Linux.** One command downloads the right binary, verifies
its checksum, names your device and starts the background service:

```sh
curl -fsSL https://raw.githubusercontent.com/osmanahmadxai/beemr/main/install.sh | sh
```

**Windows (PowerShell)**

```powershell
irm https://raw.githubusercontent.com/osmanahmadxai/beemr/main/install.ps1 | iex
```

### Package managers

**Debian, Ubuntu, Mint, Pop!_OS**

```sh
curl -fsSL https://osmanahmadxai.github.io/beemr/beemr.gpg | sudo tee /usr/share/keyrings/beemr.gpg >/dev/null
echo "deb [signed-by=/usr/share/keyrings/beemr.gpg] https://osmanahmadxai.github.io/beemr/apt stable main" | sudo tee /etc/apt/sources.list.d/beemr.list
sudo apt update && sudo apt install beemr
```

**Fedora, RHEL, Rocky, Alma**

```sh
sudo curl -fsSL https://osmanahmadxai.github.io/beemr/rpm/beemr.repo -o /etc/yum.repos.d/beemr.repo
sudo dnf install beemr
```

**openSUSE**

```sh
sudo zypper addrepo https://osmanahmadxai.github.io/beemr/rpm/beemr.repo
sudo zypper install beemr
```

| Platform | Command |
|---|---|
| Snap (any distro with snapd) | `sudo snap install beemr` |
| Arch, Manjaro (AUR) | `yay -S beemr-bin` |
| Homebrew (macOS, Linux) | `brew install osmanahmadxai/beemr/beemr` |
| Scoop (Windows) | `scoop bucket add beemr https://github.com/osmanahmadxai/scoop-beemr` then `scoop install beemr` |
| Alpine | `apk add --allow-untrusted beemr_*.apk` from [releases](https://github.com/osmanahmadxai/beemr/releases/latest) |
| Anything else | static binaries on the [releases page](https://github.com/osmanahmadxai/beemr/releases/latest) |

After installing with a package manager, run `beemr setup` once to name the
device and start the background service that receives messages.

The APT and RPM repositories are signed with key
`5B5C C4E3 B2D2 FD72 1EA8 ECC9 2FF3 31B1 D566 DAFA`, and updates arrive
through your normal system updates. Nothing else is required: no Docker, no
runtime, no account.

## Uninstall

| Installed with | Remove with |
|---|---|
| macOS / Linux installer | `curl -fsSL https://raw.githubusercontent.com/osmanahmadxai/beemr/main/uninstall.sh \| sh` |
| Windows installer | `irm https://raw.githubusercontent.com/osmanahmadxai/beemr/main/uninstall.ps1 \| iex` |
| apt / dnf / zypper | `sudo apt remove beemr` · `sudo dnf remove beemr` · `sudo zypper remove beemr` |
| Snap / Homebrew / Scoop | `sudo snap remove beemr` · `brew uninstall beemr` · `scoop uninstall beemr` |

Run `beemr daemon uninstall` first to stop the background service. The
uninstall scripts do this for you. Your identity, contacts and inbox are kept
unless you set `BEEMR_PURGE=1` when running an uninstall script.

## Usage

### Files

```sh
beemr share report.pdf                 # anyone with the printed command can download it once
beemr share photos/ -n 3 -e 2h         # a folder, up to 3 downloads within 2 hours
beemr share contract.pdf --to sara     # only Sara's device can download it
beemr get <ticket>                     # download, into the current folder
beemr get <ticket> -o ~/Downloads
```

### Messages

```sh
beemr id                               # your device ID: give it to people who want to reach you
beemr contact add Sara <her-device-id> # save a device under a name
beemr msg sara "are you free tonight?"
beemr inbox                            # read messages (unknown senders are clearly marked)
beemr reply 1 "yes, 8pm"
```

If the other device is offline, the message is queued and delivered
automatically when you're both online. Messages arrive through the background
service that `beemr setup` starts. Check it with `beemr daemon status`.

### This device

```sh
beemr setup                # name this device and start the background service
beemr name "Osman's Mac"   # rename this device
beemr doctor               # how reachable is this device, and why
```

## How it connects

beemr tries every path at once and keeps the best:

| Step | What happens | Needs |
|---|---|---|
| 1. Direct | LAN, IPv6, or a port your router opens via UPnP | nothing |
| 2. Hole punching | Both devices connect at the same moment through their firewalls, coordinated through a public relay | nothing |
| 3. beemr relay | Another beemr device that is reachable from the internet forwards the encrypted data | at least one reachable beemr device online |

Hole punching works on most home and mobile networks. When **both** devices
sit behind the strictest kind of NAT (symmetric NAT, common on some carriers),
some reachable third machine has to forward the traffic. No app can avoid
that. beemr makes that machine another beemr user, never a company
server. Any beemr device that is reachable from the internet (a router with
UPnP, IPv6, a public IP) relays automatically while its background service
runs. You can also run a dedicated relay on any machine:

```sh
beemr relay                         # relay for everyone (listed on the DHT)
beemr relay --private               # relay only for devices you configure
beemr relay use <address>           # use a specific relay
```

Run `beemr doctor` to see which steps work on your network.

## Security

- **Encryption.** Connections use libp2p's Noise protocol (X25519 and
  ChaCha20-Poly1305) or TLS 1.3 over QUIC, end to end, including through relays.
- **Identity.** Each device proves its identity by signing the specific
  connection with its Ed25519 device key, so neither a relay nor anyone in
  the middle can impersonate it.
- **Tickets.** A ticket contains a one-time secret. Without it, a share can't
  be found or downloaded.
- **Safe receiving.** Received files land in a hidden staging folder and are
  moved into place only when complete. Existing files are never overwritten,
  and paths are checked so nothing can be written outside the download folder.
- **Names are self-chosen.** Unless you've saved a device as a contact, its
  name is shown as unverified.

beemr stores its identity, contacts and inbox in `~/.config/beemr/`
(Linux), `~/Library/Application Support/beemr/` (macOS) or
`%APPDATA%\beemr\` (Windows). Set `BEEMR_HOME` to use another folder.

The wire format is specified in [PROTOCOL.md](PROTOCOL.md).

## Privacy policy

beemr has no telemetry and sends nothing to the project or its maintainers.
To let other devices reach you, it publishes a signed record with this
device's ID, its chosen name and its current network addresses on the public
BitTorrent DHT. It also connects to public IPFS nodes to find relays for hole
punching. Files and messages travel only between the devices involved,
end-to-end encrypted. Your identity, contacts and messages are stored only on
your device.

## Code signing policy

Free code signing provided by [SignPath.io](https://about.signpath.io/),
certificate by [SignPath Foundation](https://signpath.org/). The application
is pending; Windows releases will be signed once it is approved.

- Committers and reviewers: [Osman Ahmadzai](https://github.com/osmanahmadxai)
- Approvers: [Osman Ahmadzai](https://github.com/osmanahmadxai)

Every signed release is built by GitHub Actions from this repository's public
source and approved by hand before signing.

## Building from source

Needs Rust 1.89 or newer.

```sh
cargo build --release            # binary in target/release/beemr
cargo test                       # unit tests + end-to-end tests (no internet needed)
cargo clippy --all-targets -- -D warnings
```

`tests/docs-smoke.sh` runs every command in this README against a real build,
as several devices on one machine (86 checks). `tests/natlab/run.sh` builds two
simulated home networks behind real Linux NAT routers in Docker and checks
hole punching, the relay fallback through symmetric NATs, and the error shown
when no relay exists. CI runs all of these on every push.

For troubleshooting, `BEEMR_LOG=debug beemr …` prints detailed network logs.

| Module | Responsibility |
|---|---|
| `main.rs` | Command-line interface |
| `node.rs` | libp2p node: QUIC/TCP, Noise, UPnP, AutoNAT, relays, hole punching |
| `connect.rs` | The connection ladder: direct → hole punching → relay |
| `discovery.rs` | Signed address records on the Mainline DHT |
| `proto.rs` | The beemr stream protocol |
| `share.rs` / `get.rs` | Sending and receiving files |
| `message.rs` / `daemon.rs` | Messages, inbox and the background service |
| `service.rs` | Starting at login on macOS, Linux and Windows |
| `relay.rs` / `doctor.rs` | Dedicated relays and connectivity diagnostics |

## License

MIT
