# pulsesend CLI

Send and inspect PulseSend emails from the terminal. Single static binary, Rust.

User docs: https://docs.pulse-send.jiordiviera.me/cli (source: `docs/cli.mdx`).

```bash
curl -fsSL https://pulse-send.jiordiviera.me/install.sh | sh
```

## Développer

```bash
cd cli
cargo run -- --help
PULSESEND_BASE_URL=https://pulsesend.test cargo run -- whoami   # contre l'app locale (lerd)
cargo test
```

- `src/main.rs` : commandes (clap) — login, logout, whoami, send, emails list/get.
- `src/api.rs` : client HTTP (ureq + rustls) ; certificats intégrés + ceux du système (hors macOS).
- `src/config.rs` : clé enregistrée dans `~/.config/pulsesend/config.json` (0600).

## Publier une version

1. Monter `version` dans `Cargo.toml`.
2. `cli/scripts/release.sh` construit les 5 cibles (Linux x86_64/ARM64 musl, macOS Intel/Apple Silicon, Windows) dans `cli/dist/<version>/`, avec une empreinte SHA-256 par archive.
3. `cli/scripts/release.sh --upload` publie dans le bucket R2 public `pulse-send` sous `cli/<version>/`, puis `cli/latest.txt` en dernier (clés `R2_*` du `.env` de l'app).

Prérequis : rustup, `cargo install cargo-zigbuild`, zig dans le `PATH` (`pip install ziglang` dans un venv, ici `~/.local/share/pulsesend-zig`), zip.

Le script d'installation est `public/install.sh` de l'app (servi sur `https://pulse-send.jiordiviera.me/install.sh`) : il ne change qu'avec un déploiement.
