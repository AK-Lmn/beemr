<p align="center">
  <img src="docs/beemr-logo.png" alt="beemr" width="560">
</p>

<p align="center">
  <b>Send files and folders straight to another device.</b><br>
  No servers. No accounts. No setup. End-to-end encrypted.
</p>

<p align="center">
  <a href="https://github.com/osmanahmadxai/beemr/actions/workflows/ci.yml"><img src="https://github.com/osmanahmadxai/beemr/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/osmanahmadxai/beemr/releases/latest"><img src="https://img.shields.io/github/v/release/osmanahmadxai/beemr" alt="Latest release"></a>
  <a href="https://snapcraft.io/beemr"><img src="https://img.shields.io/snapcraft/v/beemr/latest/stable?label=snap" alt="Snap Store"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue" alt="MIT license"></a>
</p>

<p align="center">
  <a href="#install">Install</a> ·
  <a href="#usage">Usage</a> ·
  <a href="#how-beemr-compares">Compare</a> ·
  <a href="#how-it-connects">How it connects</a> ·
  <a href="#faq">FAQ</a> ·
  <a href="PROTOCOL.md">Protocol</a>
</p>

<p align="center">
  <img src="docs/demo.gif" alt="beemr sharing a folder from one device and downloading it on another" width="900">
</p>

---

## How it works

**1. Share** a file or folder. beemr prints a command:

```console
$ beemr share vacation-photos/
Sharing "vacation-photos" (214 files, 1.3 GB)

Who can download:  anyone with the command below
Downloads allowed: 1
Expires:           never

On the other device, run:

    beemr get AlTRKOzvjlzG5IBDDhhzcwj56mci3evFmCm22FhZ8NVAOe29ocweZCkDP1kmbFeakaMFBMCoARc

  ✓ Reachable on your local network
  ✓ Hole punching ready (through firewalls, no setup needed)
```

**2. Run that command** on the other device, anywhere in the world:

```console
$ beemr get AlTRKOzvjlzG5IBDDhhzcwj56mci3evFmCm22FhZ8NVAOe29ocweZCkDP1kmbFeakaMFBMCoARc
Receiving "vacation-photos" (214 files, 1.3 GB) from Osman
  via direct connection (through the firewall via hole punching)
  Receiving [========================] 100.0%  1.3 GB / 1.3 GB  48.2 MB/s
Saved to ./vacation-photos
```

That's it. The files go straight from one device to the other.

## Why beemr

| | |
|---|---|
| 🌍 **Works across the internet** | Connects directly, punches through home and mobile NATs, and falls back to a relay only when it has to. No router setup. |
| 🚫 **No servers, no accounts** | Devices find each other on the public BitTorrent DHT. Nothing is uploaded anywhere. |
| 🔒 **End-to-end encrypted** | Every byte is encrypted between the two devices, even through a relay. |
| 🪪 **Share with one device only** | Every device has a cryptographic ID. `--to sara` means only Sara's device can download. |
| ⏱️ **Limits built in** | One download by default. Allow more with `-n 5`, or expire with `-e 10m`. |
| 🪶 **Small and quiet** | One ~9 MB native binary for macOS, Linux and Windows. Nothing runs in the background. |

## How beemr compares

| | **beemr** | croc | magic-wormhole | LocalSend |
|---|---|---|---|---|
| Works across the internet | ✅ | ✅ | ✅ | ❌ same network only |
| Servers run by the project | **none** | public relay by default | rendezvous and relay servers | none |
| Direct connections through NATs (hole punching) | ✅ | ❌ internet transfers go through the relay | ✅ when possible, else relay | — |
| Share with one specific device only | ✅ `--to` | ❌ | ❌ | ❌ |
| Download limits and expiry | ✅ | ❌ | one-time code | ❌ |
| Interface | command line | command line | command line | graphical app |

All four are good tools. Choose beemr when you want transfers that go straight
between devices over the internet, with no service in between and control over
who can download. LocalSend is great on a single network, and croc or
magic-wormhole when you're fine with a relay server. Corrections are welcome.

## Install

| Platform | Command |
|---|---|
| **macOS & Linux** | `curl -fsSL https://raw.githubusercontent.com/osmanahmadxai/beemr/main/install.sh \| sh` |
| **Windows** (PowerShell) | `irm https://raw.githubusercontent.com/osmanahmadxai/beemr/main/install.ps1 \| iex` |
| **Ubuntu & any distro with snap** | `sudo snap install beemr` |
| **Homebrew** (macOS & Linux) | `brew install osmanahmadxai/beemr/beemr` |
| **Scoop** (Windows) | `scoop bucket add beemr https://github.com/osmanahmadxai/scoop-beemr` then `scoop install beemr` |

The installers verify the download's checksum and ask you to name your device.
After installing any other way, run `beemr setup` once to do that.

<details>
<summary><b>Debian, Ubuntu, Mint, Pop!_OS (apt)</b></summary>

```sh
curl -fsSL https://osmanahmadxai.github.io/beemr/beemr.gpg | sudo tee /usr/share/keyrings/beemr.gpg >/dev/null
echo "deb [signed-by=/usr/share/keyrings/beemr.gpg] https://osmanahmadxai.github.io/beemr/apt stable main" | sudo tee /etc/apt/sources.list.d/beemr.list
sudo apt update && sudo apt install beemr
```
</details>

<details>
<summary><b>Fedora, RHEL, Rocky, Alma (dnf)</b></summary>

```sh
sudo curl -fsSL https://osmanahmadxai.github.io/beemr/rpm/beemr.repo -o /etc/yum.repos.d/beemr.repo
sudo dnf install beemr
```
</details>

<details>
<summary><b>openSUSE (zypper)</b></summary>

```sh
sudo zypper addrepo https://osmanahmadxai.github.io/beemr/rpm/beemr.repo
sudo zypper install beemr
```
</details>

<details>
<summary><b>Alpine, Arch, or a plain binary</b></summary>

Download the `.apk`, `.pkg.tar.zst`, `.deb`, `.rpm` or a standalone binary
from the [latest release](https://github.com/osmanahmadxai/beemr/releases/latest).
For example: `sudo pacman -U beemr-*-x86_64.pkg.tar.zst`, or
`apk add --allow-untrusted beemr_*.apk`.
</details>

The APT and RPM repositories are signed with key
`5B5C C4E3 B2D2 FD72 1EA8 ECC9 2FF3 31B1 D566 DAFA`, and updates arrive
through your normal system updates.

## Usage

### Share and download

```sh
beemr share report.pdf                 # anyone with the printed command can download it, once
beemr share photos/ -n 3 -e 2h         # a folder: up to 3 downloads within 2 hours
beemr share contract.pdf --to sara     # only Sara's device can download it
beemr get <ticket>                     # download into the current folder
beemr get <ticket> -o ~/Downloads      # download into a specific folder
```

| Share option | What it does |
|---|---|
| `--to <contact-or-id>` | Only this device may download. Repeat for several devices. |
| `-n`, `--downloads <n>` | Number of downloads allowed. Default `1`; `0` means unlimited. |
| `-e`, `--expires <time>` | Stop accepting downloads after a time such as `30s`, `10m`, `2h` or `1d`. |
| `-p`, `--port <port>` | Listen on a specific port instead of a random one. |
| `--no-upnp` | Don't ask your router to open a port. |

### Your device and contacts

```sh
beemr setup                            # name this device
beemr id                               # your device ID, for people who send you files with --to
beemr name "Osman's Mac"               # rename this device
beemr contact add Sara <her-device-id> # save a device under a name, then use --to sara
beemr contact list                     # list saved contacts
beemr doctor                           # check how reachable this device is, and why
```

When a device you haven't saved as a contact connects, beemr shows its
self-chosen name as unverified.

## How it connects

beemr tries every path at once and keeps the best one:

| Step | What happens | Setup needed |
|---|---|---|
| **1. Direct** | Same network, IPv6, or a port your router opens via UPnP | None |
| **2. Hole punching** | Both devices connect at the same moment through their firewalls, coordinated by public peer-to-peer nodes | None |
| **3. Relay** | A beemr relay that's reachable from the internet forwards the encrypted data | A relay online (see below) |

Hole punching works on most home and mobile networks. If **both** devices
sit behind the strictest kind of NAT (symmetric NAT, used by some mobile
carriers), a reachable third machine has to forward the traffic. No app can
avoid that. With beemr, that machine is a relay anyone can run, never a
company server. Any computer that's reachable from the internet can be one: a
router with UPnP, IPv6, a public IP, or a small cloud server.

```sh
beemr relay                  # run a relay for everyone (beemr devices find it automatically)
beemr relay --private        # run a relay only for devices you configure
beemr relay use <address>    # always try a specific relay
```

## FAQ

<details>
<summary><b>Where do downloaded files go?</b></summary>

Into the folder you run `beemr get` from, or the folder given with `-o`.
beemr prints the exact path when it finishes. It never overwrites a file;
a second copy is saved as `name (1).ext`.
</details>

<details>
<summary><b>Do both devices need to be online at the same time?</b></summary>

Yes. Files go directly from one device to the other, and nothing is stored on
a server in between. Keep `beemr share` running until the other side has
downloaded.
</details>

<details>
<summary><b>Windows Defender says beemr is a virus</b></summary>

New programs that aren't code-signed yet sometimes get flagged by Windows
Defender's machine-learning heuristics. That's a false positive. To keep
beemr, open **Windows Security → Virus & threat protection → Protection
history**, select beemr and choose **Allow on device**. Windows builds will be
code-signed (see [Code signing policy](#code-signing-policy)), which stops
these warnings.
</details>

<details>
<summary><b>What does someone learn if they get hold of my ticket?</b></summary>

A ticket lets its holder download that one share, until the download limit or
expiry is reached. For anything sensitive, use `--to` so only a specific
device can download, even with the ticket.
</details>

<details>
<summary><b>It can't connect. What now?</b></summary>

Run `beemr doctor` on both devices. It shows which connection paths work on
each network. If both are behind strict NATs, run `beemr relay` on any machine
that's reachable from the internet.
</details>

## Uninstall

| Installed with | Remove with |
|---|---|
| macOS / Linux installer | `curl -fsSL https://raw.githubusercontent.com/osmanahmadxai/beemr/main/uninstall.sh \| sh` |
| Windows installer | `irm https://raw.githubusercontent.com/osmanahmadxai/beemr/main/uninstall.ps1 \| iex` |
| Snap / Homebrew / Scoop | `sudo snap remove beemr` · `brew uninstall beemr` · `scoop uninstall beemr` |
| apt / dnf / zypper | `sudo apt remove beemr` · `sudo dnf remove beemr` · `sudo zypper remove beemr` |

Your device identity and contacts are kept unless you set `BEEMR_PURGE=1`
when running an uninstall script. They live in `~/.config/beemr/` (Linux),
`~/Library/Application Support/beemr/` (macOS) or `%APPDATA%\beemr\`
(Windows).

## Security and privacy

- **Encryption.** Connections use libp2p's Noise protocol (X25519 and
  ChaCha20-Poly1305) or TLS 1.3 over QUIC, end to end, including through relays.
- **Identity.** Each device proves who it is by signing the specific
  connection with its Ed25519 device key, so neither a relay nor anyone in the
  middle can impersonate it.
- **Safe receiving.** Files land in a hidden staging folder and appear under
  their real name only when complete. Existing files are never overwritten,
  and paths are checked so nothing can be written outside the download folder.

### Privacy policy

beemr has no telemetry and sends nothing to the project or its maintainers.
While you share something, beemr publishes a signed record with this
device's ID, its chosen name and its current network addresses on the public
BitTorrent DHT, so the receiver can find it. It also connects to public IPFS
nodes to find relays for hole punching. Files travel only between the devices
involved, end-to-end encrypted. Your identity and contacts are stored only on
your device.

### Code signing policy

Free code signing provided by [SignPath.io](https://about.signpath.io/),
certificate by [SignPath Foundation](https://signpath.org/). The application
is pending; Windows releases will be signed once it's approved.

- Committers and reviewers: [Osman Ahmadzai](https://github.com/osmanahmadxai)
- Approvers: [Osman Ahmadzai](https://github.com/osmanahmadxai)

Every signed release is built by GitHub Actions from this repository's public
source and approved by hand before signing.

## Development

Needs Rust 1.89 or newer.

```sh
cargo build --release                       # binary in target/release/beemr
cargo test                                  # unit and end-to-end tests (no internet needed)
cargo clippy --all-targets -- -D warnings   # lints
tests/docs-smoke.sh                         # runs every command in this README
```

`tests/natlab/run.sh` builds two simulated home networks behind real Linux NAT
routers in Docker. It checks hole punching, the relay fallback through
symmetric NATs, and the error shown when no relay exists. CI runs all of these
on every push, and every release is test-installed on Debian, Ubuntu, Fedora,
Rocky, AlmaLinux, openSUSE, Alpine, Arch and the Snap Store.

For troubleshooting, `BEEMR_LOG=debug beemr …` prints detailed network logs.
The wire protocol is specified in [PROTOCOL.md](PROTOCOL.md).

<details>
<summary><b>Source layout</b></summary>

| Module | Responsibility |
|---|---|
| `main.rs` | Command-line interface |
| `node.rs` | libp2p node: QUIC/TCP, Noise, UPnP, AutoNAT, relays, hole punching |
| `connect.rs` | The connection ladder: direct → hole punching → relay |
| `discovery.rs` | Signed address records on the Mainline DHT |
| `proto.rs` | The beemr stream protocol |
| `share.rs` / `get.rs` | Sending and receiving files |
| `relay.rs` / `doctor.rs` | Dedicated relays and connectivity diagnostics |
| `service.rs` | Removes the background service left by versions before 0.3 |
</details>

## License

[MIT](LICENSE)
