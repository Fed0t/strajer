# Validare milestone M0

Data: 30 august 2026.

## Mediu

- macOS pe MacBook Pro M4 Pro;
- Warcraft III Reforged `2.0.4.23745 x86_64`;
- Docker Engine `29.7.2`;
- Rust `1.92.0`.

## Verificări reușite

- `cargo fmt --all --check`;
- `cargo test --workspace --locked`: 12 teste trecute;
- `cargo clippy --workspace --all-targets -- -D warnings`;
- build Linux al imaginii `strajer-strajer-server` din `Cargo.lock`;
- container `healthy`, UID/GID `10001:10001`, root filesystem read-only, toate capabilities eliminate și `no-new-privileges` activ;
- `/healthz`, `/readyz` și `/v1/lobbies` răspund corect;
- Bonjour găsește `Strajer Test #1` pe `_blizzard._udp` și pe subtype-ul `_w3xp2774`;
- publisher-ul folosește `Interface::LocalOnly`; la runtime `dns-sd` raportează
  exact o instanță pe interface `-1`, fără reclamă pe interfețele LAN;
- query-ul DNS pentru service instance returnează recordul type `66`, 356 bytes;
- Warcraft afișează în `Local Network Games`: `1/24`, `Strajer Test #1`, `Synthetic.w3x`.
- `Strajer.app` pornește agentul ca proces copil și publică același lobby fără agent CLI separat;
- executabilele SwiftUI și Rust din bundle conțin ambele arhitecturi: `arm64` și `x86_64`;
- semnătura ad-hoc, ZIP-ul extras și checksum-ul SHA-256 au fost validate.

Mesajul Warcraft `Could not find this map file on your computer` este așteptat: milestone-ul folosește intenționat un descriptor sintetic și nu distribuie încă o hartă.

## Încă nevalidate

- vizibilitatea simultană pe ambele Mac-uri aflate din nou în același LAN, cu
  pachetul care include fixul `LocalOnly`;
- pachetul `REQJOIN` real și forwarding-ul către Linux;
- map availability, lobby W3GS și pornirea jocului;
- confirmarea pe al doilea Mac, Developer ID signing și notarizarea Apple.
