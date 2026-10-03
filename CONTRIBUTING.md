# Contributing to beemr

Thanks for helping! Bug reports, ideas and pull requests are all welcome.

## Reporting a bug

Open an [issue](https://github.com/osmanahmadxai/beemr/issues) with:

- what you ran, what you expected, and what happened
- `beemr --version` and your operating system
- the output of `beemr doctor` on both devices, for connection problems
- if possible, a log: `BEEMR_LOG=debug beemr …` (remove anything private)

Security problems: please follow [SECURITY.md](SECURITY.md) instead of
opening a public issue.

## Getting started

You need Rust 1.89 or newer.

```sh
git clone https://github.com/osmanahmadxai/beemr
cd beemr
cargo build
cargo test
```

The tests don't need the internet: the end-to-end tests run several "devices"
on your machine with a private DHT.

New here? Issues labelled
[good first issue](https://github.com/osmanahmadxai/beemr/labels/good%20first%20issue)
are a good place to start.

## Before opening a pull request

```sh
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --release && tests/docs-smoke.sh   # runs every command in the README
```

- Keep pull requests focused on one change, and explain the why in the
  description.
- Add or update tests for behaviour changes.
- Update `README.md` and the man page (`docs/beemr.1`) when you change a
  command or option.
- Changes to what goes over the network must update [PROTOCOL.md](PROTOCOL.md)
  and stay compatible with the current protocol version, or bump it.

CI runs the same checks on Linux, macOS and Windows, plus a NAT lab that tests
hole punching and relays across simulated home networks.

## Code layout

See "Source layout" at the bottom of the [README](README.md#development).
Networking lives in `node.rs` and `connect.rs`, the wire protocol in
`proto.rs`, and sharing and receiving in `share.rs` and `get.rs`.

## License

By contributing, you agree that your contributions are licensed under the
[MIT License](LICENSE).
