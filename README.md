# Strajer

Strajer este un host Warcraft III: Reforged independent de Battle.net. Un agent local macOS publică lobby-urile serverului Strajer în ecranul `Local Area Network`, iar serverul autoritativ rulează pe Linux în Docker.

## Stadiu curent

Primul vertical slice implementează:

- un control-plane HTTP containerizat;
- un lobby sintetic determinist;
- contracte versionate între server și agent;
- un canal WSS autentificat cu un token comun pentru private beta;
- un registry concurent de lobby care alocă ID-uri și sloturi și curăță determinist disconnect-urile;
- un publisher macOS DNS-SD compatibil cu formatul LAN Reforged;
- reclame Bonjour `LocalOnly`, fără coliziuni între două Mac-uri Strajer din același LAN;
- descriptor LAN verificat pentru `Maps\Download\DotA_v6_89Q.w3x`;
- un frame reader W3GS incremental și validarea sigură a `REQJOIN`;
- răspunsurile W3GS necesare intrării în lobby și sincronizarea live a roster-ului;
- un `Strajer.app` nativ SwiftUI cu iconiță în menu bar, status și agent Rust inclus.

Intrarea în lobby și sincronizarea join/leave sunt implementate. Pornirea jocului,
transportul action-urilor, map transfer-ul și replay-ul sunt milestone-uri
ulterioare. Harta trebuie instalată identic pe ambele Mac-uri. Strajer nu modifică
Warcraft III și nu publică jocuri în catalogul Battle.net.

## Dezvoltare locală

Rulează verificările Rust cu un Cargo home temporar dacă mediul Codex nu poate scrie în `~/.cargo`:

```bash
CARGO_HOME=/private/tmp/strajer-cargo-home cargo test --workspace
CARGO_HOME=/private/tmp/strajer-cargo-home cargo clippy --workspace --all-targets -- -D warnings
```

Pornește serverul local în Docker:

```bash
export STRAJER_JOIN_TOKEN="$(openssl rand -hex 32)"
docker compose up --build
```

Pornește agentul macOS:

```bash
STRAJER_SERVER_URL=http://127.0.0.1:18080 \
  STRAJER_JOIN_TOKEN="${STRAJER_JOIN_TOKEN}" \
  CARGO_HOME=/private/tmp/strajer-cargo-home \
  cargo run -p strajer-agent
```

După publicarea lobby-ului, deschide Warcraft III și intră în `Local Area Network`.

Construiește aplicația universală pentru Apple Silicon și Intel:

```bash
STRAJER_SERVER_URL=http://127.0.0.1:18080 \
  STRAJER_JOIN_TOKEN="${STRAJER_JOIN_TOKEN}" \
  scripts/build-macos-app.sh

scripts/package-macos-app.sh
open dist/Strajer.app
```

Pentru un alt Mac, build-ul trebuie să conțină endpoint-ul HTTPS public și exact
același `STRAJER_JOIN_TOKEN` ca serverul. Token-ul comun este o măsură temporară
pentru private beta; identitatea per instalare și Keychain rămân soluția de
producție.

## Documentație

- [Plan de dezvoltare](docs/development-plan.md)
- [Arhitectură](docs/architecture.md)
- [Plan pentru join W3GS real](docs/join-plan.md)
- [Sursele protocolului LAN](docs/protocol-sources.md)
- [Validarea milestone-ului M0](docs/validation-m0.md)
- [Instalare pe alt Mac](docs/install-macos.md)
- [Deploy Linux și Nginx Proxy Manager](docs/deploy-linux.md)
- [Notificări third-party](THIRD_PARTY_NOTICES.md)
