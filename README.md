# pulsesend CLI

Send and inspect transactional email with [PulseSend](https://send.noburi.space) from your terminal. One static binary, no dependencies.

```bash
curl -fsSL https://send.noburi.space/install.sh | sh
```

Or with Homebrew:

```bash
brew install useburoo/tap/pulsesend
```

Windows: download `pulsesend-<version>-x86_64-pc-windows-msvc.zip` from the [latest release](https://github.com/useburoo/pulsesend-cli/releases/latest) and put `pulsesend.exe` on your `PATH`.

## Usage

```bash
pulsesend login                       # paste an API key from the dashboard
pulsesend send --from "Acme <hello@acme.com>" --to customer@example.com \
  --subject "Your code is 482910" --text "Your code is 482910" --wait
pulsesend emails list
pulsesend emails get em_01j9z…
pulsesend batch emails.json           # up to 100 emails, all or nothing
pulsesend domains add acme.com        # DNS records to publish, then: domains verify acme.com --wait
pulsesend webhooks add --url https://acme.com/hooks/pulsesend
```

`--json` on any command prints the raw API response. In a terminal the CLI animates its steps; in a pipe or CI it prints plain lines, and `NO_COLOR` turns colors off.

Full documentation: https://docs.send.noburi.space/cli

## Development

```bash
cargo run -- --help
PULSESEND_BASE_URL=https://pulsesend.test cargo run -- whoami   # against a local PulseSend
cargo test && cargo clippy --all-targets -- -D warnings
```

- `src/main.rs`: commands (clap)
- `src/api.rs`: HTTP client (ureq + rustls, bundled roots plus the system's outside macOS)
- `src/ui.rs`: spinners, steps, colors, error box
- `src/config.rs`: saved key in `~/.config/pulsesend/config.json` (mode 0600)

## Releases

Pushing a `v*` tag runs `.github/workflows/release.yml`: native builds for Linux (x86_64, ARM64, static musl), macOS (Intel, Apple Silicon) and Windows, each smoke-tested, then a GitHub release, the same archives on `media.send.noburi.space/cli/<version>/` for the install script, and the Homebrew formula in [useburoo/homebrew-tap](https://github.com/useburoo/homebrew-tap). The tag must match `version` in `Cargo.toml`.

This repository mirrors the `cli/` folder of the PulseSend monorepo.

## License

MIT
