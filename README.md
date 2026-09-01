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
- un host virtual `Strajer` în slotul `HOSTBOT`, separat de cele 10 sloturi umane;
- chat lobby bidirectional si exact-once intre jucatorii conectati, inclusiv
  echo autoritativ catre expeditor, cu identitatea validata de agent si derivata
  server-side din sesiunea autentificata;
- confirmare de hartă per client și countdown server-side de 60 secunde, cu mesaj
  W3GS la fiecare 10 secunde, anulare la leave și tranziție sincronă spre loading;
- start automat la 10/10 jucatori sau fallback `!start` de la minimum doi
  jucatori conectati, dupa ce toti au terminat verificarea hartii;
- sincronizare autoritativa `GAMELOADED_SELF`/`GAMELOADED_OTHERS` pentru
  jucatorii umani dupa ce fiecare Warcraft termina bara de loading;
- data-plane W3GS binar peste acelasi WSS, cu secvente monotone, batching
  autoritativ la 100 ms, `INCOMING_ACTION`/`INCOMING_ACTION2`, keepalive si
  detectare de desync;
- leave/disconnect in-game cu reason code W3GS, game-over si reset determinist
  al lobby-ului dupa inchiderea tuturor sesiunilor;
- stocare de hartă read-only pe server, download HTTPS autentificat, cache local
  verificat și transfer W3GS către Warcraft când harta lipsește;
- un `Strajer.app` nativ SwiftUI cu iconiță în menu bar, status, nickname persistent
  și agent Rust inclus.

Intrarea in lobby, map download-ul si fluxul autoritativ
`join -> countdown -> loading -> gameplay -> cleanup` sunt implementate.
Aplicatia supravegheaza agentul cu backoff exponential bounded si jitter,
republica lobby-urile dupa schimbarea retelei si roteste automat logul local.
Fiecare conexiune Warcraft are un ID local corelat cu statusul din menu bar, iar
inchiderea unei sesiuni sterge numai starea acelui join. WSS foloseste heartbeat
bidirectional si watchdog-uri bounded: agentul detecteaza lipsa serverului in
maximum aproximativ 35 secunde, iar serverul elibereaza un client half-open in
maximum aproximativ 45 secunde.
Action loop-ul si lifecycle-ul sunt validate automat; validarea live de 15
minute pe doua Mac-uri si replay-ul `.w3g` raman gate-uri deschise. Harta nu mai trebuie preinstalată pe Mac: serverul o
distribuie agentului, iar agentul o livrează către Warcraft prin protocolul W3GS.
Strajer nu modifică binarul sau arhiva CASC Warcraft III și nu publică jocuri în
catalogul Battle.net. Pentru regresia offline Reforged, aplicația instalează un
override WebUI local, derivat și verificat din versiunea instalată pe același Mac.

## Dezvoltare locală

Rulează verificările Rust cu un Cargo home temporar dacă mediul Codex nu poate scrie în `~/.cargo`:

```bash
CARGO_HOME=/private/tmp/strajer-cargo-home cargo test --workspace
CARGO_HOME=/private/tmp/strajer-cargo-home cargo clippy --workspace --all-targets -- -D warnings
```

Pornește serverul local în Docker:

```bash
mkdir -p maps
# Pune maps/DotA_v6_89Q.w3x înainte de pornire.
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
Cu doua Mac-uri, scrie `!start` in chatul lobby-ului. Daca verificarea hartii nu
s-a terminat pe ambele, cererea ramane armata si countdown-ul porneste automat
cand ambii jucatori devin ready.

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
- [Runbook Private Beta](docs/private-beta-runbook.md)
- [Review și gate-uri de producție](docs/production-review.md)
- [Notificări third-party](THIRD_PARTY_NOTICES.md)
