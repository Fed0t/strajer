# Strajer

Strajer este un host Warcraft III: Reforged independent de Battle.net. Un agent local macOS publică lobby-urile serverului Strajer în ecranul `Local Area Network`, iar serverul autoritativ rulează pe Linux în Docker.

## Stadiu curent

Primul vertical slice implementează:

- un control-plane HTTP containerizat;
- un lobby sintetic determinist;
- contracte versionate între server și agent;
- un publisher macOS DNS-SD compatibil cu formatul LAN Reforged;
- un listener local pregătit să captureze primul pachet trimis de `Join`;
- un `Strajer.app` nativ SwiftUI cu iconiță în menu bar, status și agent Rust inclus.

Host engine-ul W3GS, map transfer-ul și pornirea jocului sunt milestone-uri ulterioare. Strajer nu modifică Warcraft III și nu publică jocuri în catalogul Battle.net.

## Dezvoltare locală

Rulează verificările Rust cu un Cargo home temporar dacă mediul Codex nu poate scrie în `~/.cargo`:

```bash
CARGO_HOME=/private/tmp/strajer-cargo-home cargo test --workspace
CARGO_HOME=/private/tmp/strajer-cargo-home cargo clippy --workspace --all-targets -- -D warnings
```

Pornește serverul local în Docker:

```bash
docker compose up --build
```

Pornește agentul macOS:

```bash
STRAJER_SERVER_URL=http://127.0.0.1:18080 \
  CARGO_HOME=/private/tmp/strajer-cargo-home \
  cargo run -p strajer-agent
```

După publicarea lobby-ului, deschide Warcraft III și intră în `Local Area Network`.

Construiește aplicația universală pentru Apple Silicon și Intel:

```bash
STRAJER_SERVER_URL=http://127.0.0.1:18080 \
  scripts/build-macos-app.sh

scripts/package-macos-app.sh
open dist/Strajer.app
```

Pentru un alt Mac, `STRAJER_SERVER_URL` trebuie să fie un endpoint accesibil de acel Mac, nu `127.0.0.1`.

## Documentație

- [Plan de dezvoltare](docs/development-plan.md)
- [Arhitectură](docs/architecture.md)
- [Sursele protocolului LAN](docs/protocol-sources.md)
- [Validarea milestone-ului M0](docs/validation-m0.md)
- [Instalare pe alt Mac](docs/install-macos.md)
- [Notificări third-party](THIRD_PARTY_NOTICES.md)
