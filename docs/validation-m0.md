# Validare milestone M0

Actualizat: 31 august 2026.

## Mediu

- macOS pe MacBook Pro M4 Pro;
- Warcraft III Reforged `2.0.4.23745 x86_64`;
- Docker Engine `29.7.2`;
- Rust `1.92.0`.

## Verificări reușite

- `cargo fmt --all --check`;
- `cargo test --workspace --all-targets`: 83 teste trecute pentru protocol,
  framing W3GS, LAN, server, agent, chat, countdown, load sync si map transfer;
- `cargo clippy --workspace --all-targets -- -D warnings`;
- build Linux al imaginii `strajer-strajer-server` din `Cargo.lock`;
- container `healthy`, UID/GID `10001:10001`, root filesystem read-only, toate capabilities eliminate și `no-new-privileges` activ;
- `/healthz`, `/readyz` și `/v1/lobbies` răspund corect;
- Bonjour găsește `Strajer Test #1` pe `_blizzard._udp` și pe subtype-ul `_w3xp2774`;
- publisher-ul folosește `Interface::LocalOnly`; la runtime `dns-sd` raportează
  exact o instanță pe interface `-1`, fără reclamă pe interfețele LAN;
- query-ul DNS pentru service instance returnează recordul type `66`, 356 bytes;
- baseline-ul runtime anterior afișa în `Local Network Games`: `1/24`,
  `Strajer Test #1`, `Synthetic.w3x`;
- endpoint-ul local `/v1/lobbies` publică acum descriptorul verificat pentru
  `Maps\Download\DotA_v6_89Q.w3x`;
- `Strajer.app` pornește agentul ca proces copil și publică același lobby fără agent CLI separat;
- aplicația poate afișa evenimentul non-sensibil `Join request detected` după un
  `REQJOIN` valid;
- override-ul WebUI pentru Reforged `2.0.4` înlocuiește exact cele patru trimiteri
  greșite `selectedGame` cu `selectedGameId` si cele sapte valori initiale de
  nickname; testul Swift trece pe fixture-ul real `GlueManager.js`; după restart, Join-ul offline a
  produs `REQJOIN`, `lobby_joined` și un roster coordonat de doi jucători;
- heartbeat-ul WSS la 30 s mentine lobby-ul public activ peste 128 s, depasind
  timeout-ul idle vechi de aproximativ 90 s fara reconnect sau eroare;
- doua Mac-uri au intrat simultan in acelasi lobby coordonat;
- agentul de pe Mac-ul fara asset a descarcat si verificat harta in cache;
- serverul rezerva 10 sloturi umane, iar agentul ocupa slotul final `HOSTBOT` cu
  playerul virtual `Strajer`;
- serverul testeaza countdown-ul 60/50/40/30/20/10, anularea la leave si startul
  numai dupa ready pentru toti jucatorii;
- testele end-to-end WSS valideaza chatul in ambele directii fara echo spre
  expeditor si `!start` armat inainte de ready pentru un lobby partial cu doi
  jucatori;
- serverul valideaza ca `loaded` este acceptat numai dupa start, este idempotent
  si publica fiecare PID uman o singura data; codec-ul valideaza frame-ul
  `GAMELOADED_SELF` si genereaza `GAMELOADED_OTHERS`;
- listener-ele Warcraft sunt verificate pe acelasi port, exclusiv pe loopback
  IPv4 si IPv6;
- executabilele SwiftUI și Rust din bundle conțin ambele arhitecturi: `arm64` și `x86_64`;
- semnătura ad-hoc, ZIP-ul extras și checksum-ul SHA-256 au fost validate.

## Încă nevalidate

- deploy-ul coordonat schema catalog `3` / session protocol `5`;
- afisarea live a chatului bidirectional si validarea `!start` pe ambele Mac-uri;
- afisarea live `Strajer` in `HOSTBOT` si countdown-ul chat pe ambele Mac-uri;
- tranzitia live simultana a celor 10 clienti spre loading;
- load sync uman live pe doua Mac-uri;
- action data-plane, timeslot-uri si lifecycle in-game sunt validate automat,
  dar nu inca intr-un joc live de 15 minute pe doua Mac-uri;
- replay, reconnect si comportamentul sub lag real;
- 50 de cicluri complete fara task-uri sau sloturi fantoma;
- Developer ID signing, hardened runtime și notarizarea Apple.
