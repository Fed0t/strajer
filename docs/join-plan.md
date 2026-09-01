# Plan pentru join W3GS real

## Rezultatul urmărit

Două instanțe Warcraft III Reforged, aflate în rețele diferite, trebuie să intre în
același lobby găzduit de `strajer-server` pe Linux. Fiecare client vede jocul numai
în `Local Area Network`; Strajer nu publică nimic în catalogul Multiplayer
Battle.net și nu cere port forwarding pe Mac-uri.

Există trei praguri distincte:

1. **Join coordonat**: agentul validează local `W3GS_REQJOIN`, iar serverul Linux
   alocă player ID-ul și slotul prin WSS.
2. **Real lobby join**: agentul proiectează roster-ul serverului în
   `W3GS_SLOTINFOJOIN`, `PLAYERINFO` și `MAPCHECK`, iar Warcraft deschide lobby-ul
   și afișează jucătorii și sloturile.
3. **Gameplay remote**: action-urile W3GS sunt transportate și ordonate de
   serverul autoritativ până la finalul jocului.

Toate cele trei praguri sunt implementate in vertical slice. Primele doua au
fost validate live; gameplay-ul remote este acoperit automat si asteapta gate-ul
live de 15 minute pe doua Mac-uri.

## Ce este deja verificat

- Warcraft III Reforged `2.0.4.23745 x86_64` citește lobby-ul sintetic publicat
  de Strajer prin DNS-SD.
- Fiecare agent publică acum serviciul Bonjour ca `LocalOnly`; două Mac-uri din
  același LAN nu mai concurează pentru același service instance.
- Listener-ul local folosește același port pe IPv4 și IPv6; testul automat
  confirmă conexiuni prin `127.0.0.1` și `::1`.
- Reforged `2.0.4` are o regresie WebUI în modul offline: trimite
  `selectedGame` în loc de `selectedGameId`. Override-ul local verificat repară
  exact cele patru apeluri afectate fără să modifice arhiva CASC. `Strajer.app`
  il deriva acum automat din WebUI-ul servit local de joc, valideaza semnatura si
  cere un singur restart Warcraft dupa instalare; fisierul Blizzard nu este
  inclus in distributia Strajer.
- Warcraft inițiază o conexiune TCP către portul din recordul LAN atunci când
  utilizatorul apasă `Join`.
- Agentul citește incremental primul frame W3GS și validează `REQJOIN`,
  `host_counter` și `entry_key`.
- Serverul expune un endpoint WSS per lobby, autentificat cu bearer token,
  alocă player ID-uri și sloturi și distribuie un roster versionat.
- Agentul trimite local `SLOTINFOJOIN`, slot info, `PLAYERINFO`, profile/skins
  Reforged și `MAPCHECK`, apoi aplică join/leave live.
- Serverul distribuie harta printr-un endpoint HTTPS autentificat, iar agentul o
  validează, o păstrează într-un cache atomic și implementează transferul W3GS în
  ferestre cu backpressure bazat pe `MAPSIZE`.
- UI-ul Warcraft a fost validat pe doua Mac-uri cu doua sesiuni WSS reale si
  roster comun. Valorile istorice `2/11` si `1/11` erau anterioare host-ului
  virtual; build-ul nou trebuie sa afiseze `Strajer` suplimentar in `HOSTBOT`.
- Join-ul Reforged offline a fost validat după workaround: agentul a primit
  `REQJOIN`, serverul a alocat player ID `2`, iar roster-ul a ajuns la doi
  jucători.
- Proxy-ul public inchidea WSS dupa aproximativ 90 de secunde fara trafic.
  Agentul trimite heartbeat WebSocket la 10 secunde, iar serverul la 15 secunde.
  Watchdog-ul agentului inchide sesiunea dupa 35 secunde fara trafic server-side,
  iar watchdog-ul serverului elibereaza slotul dupa 45 secunde fara trafic de la
  client. Testul public anterior a mentinut lobby-ul peste 128 de secunde; noile
  deadline-uri sunt acoperite automat si asteapta revalidarea live.

Join-ul simultan pe doua Mac-uri este validat. Urmatorul deploy trebuie sa
actualizeze coordonat serverul la catalog schema `3` / session protocol `5` si
ambele aplicatii; versiunile vechi sunt refuzate intentionat. Tranzitia spre
loading, sincronizarea all-loaded si data-plane-ul action-urilor sunt
implementate; validarea live a action loop-ului ramane deschisa.

Manifestul verificat local la 30 august 2026 este:

- cale W3GS: `Maps\Download\DotA_v6_89Q.w3x`;
- dimensiune fișier: `35.053.979` bytes;
- SHA-1 brut al arhivei, folosit drept `map_sha1`: `c771ac8d7dc3665a211c2b1432672d49bfba1bcf`;
- CRC32 brut al arhivei, necesar ulterior în `MAPCHECK`: `2194498669`;
- checksum Xoro al conținutului MPQ: `448311427`;
- dimensiuni hartă: `128x128` tiles.

Fișierul din directorul local `maps/` și copia instalată local în Warcraft sunt
byte-identice. Numai serverul trebuie să aibă asset-ul înainte de pornire; harta
nu este inclusă automat în Git, imaginea Docker sau bundle-ul aplicației.

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

Agentul este un proxy W3GS local, nu host-ul final de gameplay. Serverul Linux
detine starea lobby-ului, aloca player ID-urile si ruleaza action loop-ul
autoritativ; fiecare agent proiecteaza aceeasi stare si aceleasi timeslot-uri in
Warcraft-ul local. Toate conexiunile de Internet sunt initiate outbound de
agent.

### Discovery local

- Un listener TCP separat pentru fiecare lobby publicat.
- Un service instance DNS-SD `LocalOnly` pentru fiecare listener.
- `NO_AUTO_RENAME` rămâne activ pentru a detecta două instanțe Strajer pornite
  accidental pe același Mac.
- Publicarea `LocalOnly` este verificată; listener-ele TCP folosesc acelasi port
  dinamic, dar sunt legate exclusiv pe `127.0.0.1` si `::1`.

### Control-plane

- HTTPS pe `443/tcp`, terminat de Nginx Proxy Manager.
- Catalog HTTP și un WebSocket persistent per sesiune de lobby.
- Heartbeat WebSocket trimis de agent la 30 de secunde pentru a mentine
  sesiunea activa prin timeout-urile idle ale reverse proxy-ului.
- Bearer token comun, configurat prin `STRAJER_JOIN_TOKEN`, pentru private beta.
- Bootstrap-ul de identitate și token-ul de sesiune scurt și revocabil rămân
  hardening-ul de producție.

### Data-plane

- Pentru slice-ul curent: WSS pe `443/tcp` transporta control JSON si un
  data-plane binar directional pentru action-uri/timeslot-uri.
- Envelope-ul binar contine magic, protocol version `5`, tip directional,
  sequence number `u64` si un frame W3GS de maximum 1.460 bytes.
- Pentru producție: QUIC pe `443/udp`, direct către container, cu câte un stream
  bidirecțional per conexiune Warcraft și WSS fallback.
- Portul Warcraft `6112` nu este publicat și nu este forwardat în router.

### Distribuția hărții

- catalog schema `3` publică host-ul virtual, dimensiunea arhivei, CRC32-ul brut, SHA-1-ul și
  checksum-ul Xoro necesare pentru `MAPCHECK`;
- containerul montează hărțile read-only și validează asset-ul complet la boot;
- `GET /v1/maps/{sha1}` folosește același bearer token private-beta și răspunde
  prin streaming, fără a încărca întreaga hartă în memoria serverului;
- agentul preferă o copie Warcraft deja validă, apoi cache-ul
  `~/Library/Caches/Strajer/maps`, apoi download-ul HTTPS;
- download-ul agentului este limitat la dimensiunea manifestului, verificat
  SHA-1/CRC32 și instalat în cache prin rename atomic;
- către Warcraft, agentul folosește `STARTDOWNLOAD` (`0x3F`) și `MAPPART`
  (`0x43`) cu fragmente de maximum 1.442 bytes și o fereastră de 100 fragmente;
- `MAPSIZE` cu flag `3` avansează fereastra, iar flag `1` plus dimensiunea exactă
  închide transferul ca verificat. Timeout-urile de pregătire, progres și durată
  totală împiedică sesiuni blocate.

Codec-urile, autentificarea endpoint-ului, streaming-ul, cache-ul și fereastra de
transfer sunt acoperite de teste. Compatibilitatea completă a secvenței cu
Reforged `2.0.4.23745` trebuie confirmată live pe un Mac de pe care harta lipsește.

WSS peste TCP este acceptabil pentru handshake și primul proof-of-concept, dar
nu este alegerea finală pentru gameplay: pierderea unui segment exterior ar
bloca toate fluxurile multiplexate peste aceeași conexiune TCP. QUIC izolează
fluxurile și păstrează transportul outbound-only.

## Contractul agent–server curent

Control messages sunt versionate și au o limită strictă de dimensiune:

```text
HTTP Upgrade                 Authorization: Bearer <STRAJER_JOIN_TOKEN>
Agent -> server  Join        { protocol_version, player_name }
Agent -> server  Ready       { protocol_version }
Agent -> server  Chat        { protocol_version, message }
Agent -> server  Loaded      { protocol_version }
Agent -> server  Leave       { protocol_version, reason }
Server -> agent  Joined      { protocol_version, player_id, roster }
Server -> agent  Roster      { roster }
Server -> agent  Countdown   { remaining_seconds }
Server -> agent  CountdownCancelled {}
Server -> agent  Chat        { from_player_id, message }
Server -> agent  Notice      { message }
Server -> agent  Start       {}
Server -> agent  PlayerLoaded { player_id }
Server -> agent  PlayerLeft  { player_id, reason, roster }
Server -> agent  GameEnded   { reason }
Server -> agent  Rejected    { code }
```

Mesajele de control sunt JSON bounded la 4 KiB. Gameplay-ul foloseste mesaje
WebSocket binare bounded: agentul trimite numai `OUTGOING_ACTION` si
`OUTGOING_KEEPALIVE`, iar serverul trimite numai timeslot-uri
`INCOMING_ACTION`/`INCOMING_ACTION2`. Fiecare directie valideaza secvente
monotone si frame-ul W3GS inainte de procesare.

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
7. implementează chat si ready numai după ce join-ul de bază este stabil;
8. adaugă team/color, countdown, loading synchronization și action loop;
9. adaugă leave, lag handling, desync detection și replay.

Pasul 8 si partea de leave/desync/cleanup din pasul 9 sunt implementate.
Detectia half-open si rejoin-ul fara restart Strajer sunt implementate;
transparent session resume in gameplay, replay-ul si validarea live sub lag
raman deschise.

Map transfer-ul este implementat ca extensie izolată peste join-ul existent:
manifestul și cache-ul sunt separate de codec-ul W3GS, iar lipsa hărții nu mai
oprește agentul la startup. Testul live este păstrat separat de countdown și
action loop pentru diagnostic clar.

## Model de concurență pe server

- `LobbyActor`: proprietar unic al sloturilor, player list-ului și lifecycle-ului.
- `PlayerSession`: citește/scrie un singur stream și raportează evenimente actorului.
- canale `mpsc` bounded pentru backpressure; niciun queue nelimitat;
- cancellation token per lobby și cleanup determinist la disconnect;
- task-urile de socket nu modifică direct starea lobby-ului;
- registry in-memory în M1/M2; PostgreSQL numai pentru identitate, banlist și
  istoric după stabilizarea wire protocol-ului.

## Identitate și securitate

- Implementat pentru private beta: token comun aleator de 32–128 caractere,
  obligatoriu pe bind non-loopback, trimis ca bearer peste WSS și comparat
  constant-time pe server.
- Token-ul este inclus în `Info.plist` la build și poate fi extras; limitează
  accesul accidental, dar nu reprezintă identitate puternică.
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

Status la 31 august 2026: WSS autentificat, limitele de mesaj, timeout-urile și
rutarea strictă prin `lobby_id` sunt implementate. Protocolul `5` foloseste
mesaje binare directionale si secvente monotone pentru frame-urile de gameplay;
join-ul si controlul raman mesaje JSON separate.

### J2 — Lobby single-player și apoi two-player, 1–2 săptămâni

- actor de lobby și session actor;
- `SLOTINFOJOIN`, `PLAYERINFO`, `MAPCHECK`;
- o hartă reală servită de Linux și metadata verificată;
- download HTTPS, cache local și transfer W3GS pentru clientul fără hartă;
- cleanup complet la disconnect.

Ieșire: primul Mac intră în UI-ul lobby-ului; apoi două Mac-uri se văd reciproc,
iar 50 de cicluri join/leave nu lasă sloturi sau task-uri fantomă.

Status la 31 august 2026: single-player join și roster-ul two-player au fost
validate în UI pe un Mac, folosind o a doua sesiune WSS reală către serverul
local. Testele automate validează două sesiuni, player IDs distincte și cleanup
protejat împotriva disconnect-urilor stale. Pipeline-ul de map download este
implementat și testat automat, dar secvența completă trebuie încă validată în UI
pe Mac-ul fără hartă. Testul pe două Mac-uri și cele 50 de cicluri rămân criterii
deschise.

### J3 — Start și gameplay, 2–4 săptămâni

- slot changes, ready/countdown și loading;
- keepalive, pings, action batching și leave/lag handling;
- checksum/desync diagnostics;
- replay `.w3g`.

#### Autoritatea pentru `Start Game`

Verificarea live din Reforged `2.0.4.23745` arata o diferenta importanta intre
numele afisat ca host si rolul nativ de host. Primul player este afisat in lobby
ca `HOST`, dar clientul nu primeste butonul `Start Game`. WebUI apeleaza
`LobbyStart` numai cand evenimentul nativ `GameLobbySetup` seteaza `isHost=true`.
In Strajer, fiecare Warcraft este client W3GS al agentului local, deci niciun
Warcraft nu este host-ul nativ.

Decizia implementata pentru J3 este:

- serverul Linux ramane autoritatea unica pentru tranzitia lobby -> countdown ->
  loading -> gameplay;
- harta are 11 sloturi totale: 10 umane si slotul final `HOSTBOT`, ocupat de
  playerul virtual `Strajer` cu PID 11;
- fiecare agent trimite `ready` numai dupa ce Warcraft confirma harta completa;
- cand toate cele 10 sloturi umane sunt ocupate si ready, serverul porneste
  automat timerul de 60 secunde;
- pentru testele partiale, `!start` cere minimum doi jucatori si porneste acelasi
  timer dupa ce toate sesiunile conectate sunt ready; daca ready este inca in
  curs, comanda ramane armata;
- serverul publica evenimentele 60/50/40/30/20/10, iar fiecare agent le afiseaza
  in chat ca mesaje W3GS trimise de `Strajer`;
- orice leave inainte de start anuleaza timerul; un lobby plin si ready il poate
  porni din nou;
- la zero, agentii trimit local `W3GS_COUNTDOWN_START`, `W3GS_COUNTDOWN_END` si
  marcheaza host-ul virtual ca loaded;
- fiecare client raporteaza `W3GS_GAMELOADED_SELF`, iar serverul propaga PID-ul
  catre ceilalti agenti pentru `W3GS_GAMELOADED_OTHERS`;
- dupa all-loaded, serverul ruleaza tick-uri de 100 ms, ordoneaza action-urile,
  fragmenteaza timeslot-urile la limita W3GS si compara checksum-urile keepalive;
- leave/disconnect este propagat cu reason code, iar desync-ul sau ultimul
  jucator ramas produc game-over si cleanup determinist;
- nu fortam `isHost` printr-un patch WebUI: `LobbyStart` ar apela host engine-ul
  local inexistent si ar rupe modelul autoritativ.

Ieșire: doi jucători pornesc aceeași hartă, joacă minimum 15 minute și replay-ul
rezultat poate fi deschis, repetat în 20 de cicluri înainte de hardening.

### J4 — Transport și operare de producție, 1–2 săptămâni

- QUIC `443/udp` ca transport principal și WSS fallback;
- rate limits, quotas, reconnect și network transition tests;
- metrics, alerting, runbook și teste de restart container;
- 50 de cicluri end-to-end fără lobby-uri fantomă.

Estimările se recalculează după J0; cea mai mare necunoscută este diferența dintre
wire protocol-ul Reforged curent și implementările W3GS clasice disponibile public.

## Stadiul iterației după fixul mDNS

1. `FrameReader`, `ReqJoin`, validările și timeout-urile — implementate;
2. endpoint WSS bounded și autentificat — implementat;
3. registry server-side, player IDs, sloturi și cleanup — implementate;
4. `SLOTINFOJOIN`, `PLAYERINFO`, profile/skins, `MAPCHECK` și roster live — implementate;
5. endpoint de hartă, cache verificat și transfer W3GS — implementate, validarea
   live pe client fără hartă este următorul gate;
6. deploy public și validare simultană pe două Mac-uri — după gate-ul de hartă;
7. host virtual, chat, ready si countdown autoritativ automat/manual — implementate;
8. load sync uman — implementat si acoperit automat, validarea live urmeaza;
9. data-plane action/timeslot, keepalive/desync si lifecycle — implementat si
   acoperit automat; testul live de 15 minute si replay-ul urmeaza.

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
