# Plan pentru join W3GS real

## Rezultatul urmărit

Două instanțe Warcraft III Reforged, aflate în rețele diferite, trebuie să intre în
același lobby găzduit de `strajer-server` pe Linux. Fiecare client vede jocul numai
în `Local Area Network`; Strajer nu publică nimic în catalogul Multiplayer
Battle.net și nu cere port forwarding pe Mac-uri.

Există două praguri distincte:

1. **Remote join transport**: primul `W3GS_REQJOIN` primit local de agent ajunge
   nealterat la serverul Linux.
2. **Real lobby join**: host engine-ul răspunde cu `W3GS_SLOTINFOJOIN`, iar
   Warcraft deschide lobby-ul și afișează jucătorii și sloturile.

Milestone-ul M1 acoperă primul prag. M2 acoperă al doilea.

## Ce este deja verificat

- Warcraft III Reforged `2.0.4.23745 x86_64` citește lobby-ul sintetic publicat
  de Strajer prin DNS-SD.
- Fiecare agent publică acum serviciul Bonjour ca `LocalOnly`; două Mac-uri din
  același LAN nu mai concurează pentru același service instance.
- Warcraft inițiază o conexiune TCP către portul din recordul LAN atunci când
  utilizatorul apasă `Join`.
- Agentul citește incremental primul frame W3GS și validează prefixul tipizat
  `REQJOIN`; forwarding-ul nu este încă implementat.
- Serverul actual expune numai catalogul HTTP și endpoint-urile de health.

Catalogul publică acum harta reală `DotA_v6_89Q.w3x`; un join complet mai
necesită răspunsurile W3GS valide ale host engine-ului.

Manifestul verificat local la 30 august 2026 este:

- cale W3GS: `Maps\Download\DotA_v6_89Q.w3x`;
- dimensiune fișier: `35.053.979` bytes;
- SHA-1 brut al arhivei, folosit drept `map_sha1`: `c771ac8d7dc3665a211c2b1432672d49bfba1bcf`;
- CRC32 brut al arhivei, necesar ulterior în `MAPCHECK`: `2194498669`;
- checksum Xoro al conținutului MPQ: `448311427`;
- dimensiuni hartă: `128x128` tiles.

Fișierul din repository și copia instalată local în Warcraft sunt byte-identice.
Aceeași hartă trebuie să existe pe ambele Mac-uri la calea relativă de mai sus.
Fișierul nu este inclus automat în Git sau în bundle-ul aplicației.

## Arhitectura țintă

```text
Warcraft A ─TCP local─> strajer-agent A ─outbound TLS/QUIC─┐
                                                          │
                                                   strajer-server
                                                          │
Warcraft B ─TCP local─> strajer-agent B ─outbound TLS/QUIC─┘
                                      │
                                      └─ LobbyActor + W3GS host engine
```

Agentul este un proxy W3GS local, nu un host autoritativ. Serverul Linux deține
starea lobby-ului, alocă player ID-urile și sloturile și generează pachetele
server-to-client. Toate conexiunile de Internet sunt inițiate outbound de agent.

### Discovery local

- Un listener TCP separat pentru fiecare lobby publicat.
- Un service instance DNS-SD `LocalOnly` pentru fiecare listener.
- `NO_AUTO_RENAME` rămâne activ pentru a detecta două instanțe Strajer pornite
  accidental pe același Mac.
- Înainte de M1 trebuie verificat dacă target-ul rezolvat de Warcraft permite
  listener pe loopback. Până atunci bind-ul rămâne compatibil cu captura actuală;
  listener-ul nu trebuie expus în firewall.

### Control-plane

- HTTPS pe `443/tcp`, terminat de Nginx Proxy Manager.
- Bootstrap de identitate, catalog, token de sesiune scurt și revocabil.
- Un WebSocket persistent pentru control și fallback de transport.

### Data-plane

- Pentru M1: WSS pe `443/tcp`, deoarece funcționează prin proxy-ul existent și
  reduce variabilele în primul test end-to-end.
- Pentru producție: QUIC pe `443/udp`, direct către container, cu câte un stream
  bidirecțional per conexiune Warcraft și WSS fallback.
- Portul Warcraft `6112` nu este publicat și nu este forwardat în router.

WSS peste TCP este acceptabil pentru handshake și primul proof-of-concept, dar
nu este alegerea finală pentru gameplay: pierderea unui segment exterior ar
bloca toate fluxurile multiplexate peste aceeași conexiune TCP. QUIC izolează
fluxurile și păstrează transportul outbound-only.

## Contractul agent–server

Control messages sunt versionate și au o limită strictă de dimensiune:

```text
Hello              { protocol_version, installation_id, agent_version, wc3_build }
Authenticate       { bootstrap_or_session_token, nonce }
OpenJoin           { lobby_id, stream_id, join_nonce, reqjoin_length }
OpenAccepted       { stream_id, session_id }
OpenRejected       { stream_id, reason_code }
Data               { stream_id, sequence, payload }
HalfClose          { stream_id, direction }
Close              { stream_id, reason_code }
Ping / Pong         { monotonic_timestamp }
```

Payload-ul `Data` este binar. JSON rămâne numai pentru endpoint-uri de control
și diagnostic; nu se folosește pentru pachetele W3GS.

În implementarea WSS, `stream_id` și `sequence` asigură multiplexarea și
validarea ordinii. În QUIC, fiecare join primește propriul bidirectional stream,
iar envelope-ul păstrează `session_id`, versiunea și tipul mesajului.

Serverul acceptă numai un `lobby_id` emis de catalogul său. Protocolul nu oferă
agentului un câmp arbitrar `host:port`, evitând transformarea Strajer într-un
open proxy sau într-un vector SSRF.

## Framing și parser W3GS

Crate-ul `strajer-w3gs` nu are dependențe de Axum, DNS-SD sau UI.

Frame-ul de bază W3GS:

```text
offset  size  câmp
0       1     header = 0xF7
1       1     packet_id
2       2     frame_length, little-endian, include header-ul
4       n     payload
```

Pentru baseline-ul GHost++, `W3GS_REQJOIN` are packet ID `0x1E` și include host
counter, entry key, listen port, peer key, player name terminat cu NUL, internal
port și internal IPv4. Acesta este doar punctul de pornire: compatibilitatea cu
Reforged `2.0.4.23745` se confirmă din capturi locale înainte de a fixa schema.

Reguli obligatorii:

- validare `0xF7`, packet ID și lungime înainte de alocare;
- citire exactă a frame-ului, independent de fragmentarea TCP;
- limită hard de 65.535 bytes și limite mai mici per tip de packet;
- timeout pentru header, body și idle;
- decoder tipizat pentru câmpurile confirmate și păstrarea bytes-ilor necunoscuți;
- niciun dump complet de pachet sau player name în logurile `info`;
- fixture-uri redacted și teste pentru frame trunchiat, lungime invalidă, NUL lipsă
  și concatenarea mai multor frame-uri într-un singur read TCP;
- fuzzing pentru decoder înainte de expunerea publică a tunnel ingress-ului.

Status J0 la 30 august 2026:

- `FrameReader` tratează corect fragmentarea și coalescing-ul TCP;
- signature, minimum length și limita configurată sunt validate înainte de
  alocarea payload-ului;
- `ReqJoin` decodează prefixul confirmat și păstrează tail-ul necunoscut byte cu
  byte pentru compatibilitate Reforged;
- agentul nu loghează entry key, player name, IP sau payload hex;
- `Strajer.app` afișează `Join request detected` printr-un eveniment JSON care
  conține numai ID-ul intern al lobby-ului;
- captura unui fixture real din `2.0.4.23745` rămâne următorul gate.

## Host engine minim

Ordinea de implementare pentru primul lobby real:

1. acceptă `REQJOIN` și validează `host_counter`, `entry_key`, build-ul și lobby-ul;
2. alocă `player_id` și un slot liber;
3. răspunde cu `SLOTINFOJOIN`;
4. trimite `PLAYERINFO` pentru jucătorii existenți și propagă noul jucător;
5. trimite `MAPCHECK` cu metadata hărții reale;
6. procesează răspunsul de map availability și actualizează sloturile;
7. implementează chat/team/color/ready numai după ce join-ul de bază este stabil;
8. adaugă countdown, loading synchronization și action loop;
9. adaugă leave, lag handling, desync detection și replay.

Primul slice cere aceeași hartă preinstalată pe ambele Mac-uri. Map transfer-ul
este amânat până după ce doi jucători pot intra stabil în lobby; altfel amestecă
problemele de join, CASC, checksum și transfer într-un singur test greu de
diagnosticat.

## Model de concurență pe server

- `LobbyActor`: proprietar unic al sloturilor, player list-ului și lifecycle-ului.
- `PlayerSession`: citește/scrie un singur stream și raportează evenimente actorului.
- canale `mpsc` bounded pentru backpressure; niciun queue nelimitat;
- cancellation token per lobby și cleanup determinist la disconnect;
- task-urile de socket nu modifică direct starea lobby-ului;
- registry in-memory în M1/M2; PostgreSQL numai pentru identitate, banlist și
  istoric după stabilizarea wire protocol-ului.

## Identitate și securitate

- secret aleator per instalare, păstrat în macOS Keychain;
- bootstrap prin HTTPS și token de sesiune cu expirare scurtă;
- `OpenJoin` legat de installation ID, lobby ID, nonce și expiry;
- TLS obligatoriu; certificate pinning se introduce împreună cu rotația cheilor;
- limită de sesiuni per installation ID și IP;
- limite pentru frame, buffered bytes, handshake time și idle time;
- close reason codes stabile, fără erori interne returnate clientului;
- loguri structurate fără token, entry key, IP intern sau player name complet;
- mTLS poate fi adăugat ulterior, dar nu este necesar pentru primul vertical slice.

## Observabilitate

Fiecare join primește `correlation_id`, `session_id` și `stream_id`.

Countere minime:

- `join_attempt_total`;
- `reqjoin_invalid_total{reason}`;
- `join_auth_rejected_total{reason}`;
- `join_open_total` și `join_close_total{reason}`;
- `w3gs_packet_total{direction,packet_id}`.

Histograme minime:

- agent–server RTT;
- timpul de la accept local la `REQJOIN` complet;
- timpul de la `REQJOIN` la primul `SLOTINFOJOIN`;
- bytes buffered per stream.

## Reverse engineering controlat

Nu se modifică binarul Warcraft. Se capturează numai traficul propriei instalări:

1. înlocuim probe-ul curent cu un frame reader care salvează opțional un fixture
   redacted numai în development mode;
2. capturăm `REQJOIN` de mai multe ori pentru build-ul instalat;
3. variem controlat player name-ul și portul pentru a izola câmpurile;
4. comparăm structura cu GHost++, `ghostpp-rs` și dissectorul Wireshark BNETP;
5. păstrăm bytes-ii necunoscuți până când semantica este confirmată;
6. repetăm fixture-urile la fiecare update Warcraft detectat.

## Faze și criterii de ieșire

### J0 — Captură și codec, 2–4 zile

- frame reader incremental în `strajer-w3gs`;
- parser `REQJOIN` și golden fixtures Reforged;
- teste de fragmentare/coalescing și fuzz target;
- build-ul Warcraft devine parte din handshake.

Ieșire: 100 de join attempts locale sunt încadrate și decodate fără panic,
over-read sau diferențe între payload-ul capturat și fixture.

### J1 — Tunnel autentificat, 4–7 zile

- identity bootstrap și token scurt;
- WSS persistent pe `443/tcp`;
- `OpenJoin`, `Data`, backpressure, timeout și close reasons;
- server-side capture și comparație SHA-256 a frame-ului.

Ieșire: `REQJOIN` de pe ambele Mac-uri ajunge byte-identic pe Linux, fără port
forwarding pe clienți și fără posibilitatea de a selecta o destinație arbitrară.

### J2 — Lobby single-player și apoi two-player, 1–2 săptămâni

- actor de lobby și session actor;
- `SLOTINFOJOIN`, `PLAYERINFO`, `MAPCHECK`;
- o hartă reală preinstalată și metadata verificată;
- cleanup complet la disconnect.

Ieșire: primul Mac intră în UI-ul lobby-ului; apoi două Mac-uri se văd reciproc,
iar 50 de cicluri join/leave nu lasă sloturi sau task-uri fantomă.

### J3 — Start și gameplay, 2–4 săptămâni

- slot changes, ready/countdown și loading;
- keepalive, pings, action batching și leave/lag handling;
- checksum/desync diagnostics;
- replay `.w3g`.

Ieșire: doi jucători pornesc aceeași hartă, joacă minimum 15 minute și replay-ul
rezultat poate fi deschis, repetat în 20 de cicluri înainte de hardening.

### J4 — Transport și operare de producție, 1–2 săptămâni

- QUIC `443/udp` ca transport principal și WSS fallback;
- rate limits, quotas, reconnect și network transition tests;
- metrics, alerting, runbook și teste de restart container;
- 50 de cicluri end-to-end fără lobby-uri fantomă.

Estimările se recalculează după J0; cea mai mare necunoscută este diferența dintre
wire protocol-ul Reforged curent și implementările W3GS clasice disponibile public.

## Prima iterație de cod după fixul mDNS

Ordinea recomandată, fără schimbări speculative în host engine:

1. `strajer-w3gs` cu `FrameHeader`, `FrameReader` și `ReqJoin` — implementat;
2. înlocuirea read-ului unic de 4 KiB cu citire framed și timeout — implementată;
3. capturarea și redactarea primului fixture real din `2.0.4.23745` — următorul gate;
4. fixează schema `REQJOIN` pe baza fixture-ului;
5. abia apoi definește envelope-ul WSS și endpoint-ul server-side.

## Surse de interoperabilitate

- [GHost++ `gameprotocol.cpp`](https://github.com/dcramer/ghostplusplus/blob/master/ghost/gameprotocol.cpp)
  pentru layout-ul clasic `REQJOIN` și răspunsurile host engine;
- [ghostpp-rs](https://github.com/Fatorin/ghostpp-rs) pentru framing și model actor,
  folosit ca referință structurală, nu ca dovadă de compatibilitate Reforged;
- [gowarcraft3 W3GS client](https://github.com/nielsAD/gowarcraft3/blob/master/cmd/w3gsclient/main.go)
  pentru o implementare independentă;
- [Wireshark BNETP/W3GS dissector](https://github.com/diegonc/packet-bnetp/blob/main/packet-bnetp-w3gs.lua)
  pentru identificarea independentă a packet ID-urilor;
- [RFC 9000](https://www.rfc-editor.org/rfc/rfc9000) pentru proprietățile QUIC.
