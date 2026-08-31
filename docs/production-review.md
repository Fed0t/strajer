# Review de productie Strajer

Data review-ului: 31 august 2026.

## Verdict

Strajer este un vertical slice functional pentru discovery LAN, join coordonat,
distributia hartii, lobby sincronizat si un action loop W3GS autoritativ. Nu
trebuie prezentat inca drept productie: data-plane-ul este validat automat, dar
nu a trecut gate-ul live de 15 minute pe doua Mac-uri si nu genereaza replay.

## Implementat si verificat automat

- discovery exclusiv `Local Area Network` prin Bonjour `LocalOnly`;
- listener Warcraft legat numai pe `127.0.0.1` si `::1`;
- catalog HTTPS si sesiune WSS autentificata;
- map download autentificat, cache atomic verificat SHA-1/CRC32 si transfer W3GS;
- 10 sloturi umane plus `Strajer` in slotul final `HOSTBOT`;
- chat lobby bidirectional, cu PID validat local si identitate derivata din
  sesiunea autentificata pe server;
- ready dupa verificarea hartii, countdown autoritativ
  `60/50/40/30/20/10`, start automat la 10/10, fallback `!start` de la doi
  jucatori, anulare la leave si tranzitie spre loading;
- coordonare idempotenta `GAMELOADED_SELF`/`GAMELOADED_OTHERS` pentru jucatorii
  umani, cu snapshot pentru recuperarea update-urilor pierdute;
- protocol de sesiune `5`, cu envelope binar directional, frame W3GS de maximum
  1.460 bytes si sequence number monoton per directie;
- actor autoritativ la 100 ms pentru `OUTGOING_ACTION` ->
  `INCOMING_ACTION`/`INCOMING_ACTION2`, cozi bounded si CRC validation;
- consensus pentru `OUTGOING_KEEPALIVE`, terminare determinista la desync,
  leave/disconnect in-game, game-over si reset de lobby;
- nickname persistent si actiunea `Nickname...` in menu bar;
- refuz strict al unui WebUI Blizzard care nu are semnatura cunoscuta exact;
- container Linux non-root, read-only, fara capabilitati si cu healthcheck;
- build macOS universal `arm64` + `x86_64`, verificat prin `codesign` ad-hoc.

Validarea curenta are 94 de teste Rust, testul Swift pe fixture-ul WebUI real,
`cargo clippy -D warnings`, verificarea Docker Compose, build-ul imaginii Docker
si verificarea bundle-ului/ZIP-ului macOS.

## P0 - gate-uri pentru un joc real

### 1. Validare live a data-plane-ului

Actorul autoritativ, WSS binar, action batching, keepalive consensus si
detectarea desync sunt implementate. Mai trebuie confirmate pe doua instalari
Reforged reale:

- avansarea identica a jocului timp de minimum 15 minute;
- checksum stream identic si comportament determinist la packet delay/drop;
- leave in loading, leave in-game si pierderea WSS;
- limitele practice de RTT si head-of-line blocking prin proxy.

QUIC ramane o optimizare conditionata de masuratori, nu un blocker implicit.

### 2. Replay si soak

Game-over si resetul camerei sunt implementate pentru leave, disconnect,
last-player-standing si desync. Lipsesc replay-ul `.w3g`, testul a 50 de cicluri
consecutive si verificarea absentei task-urilor/sesiunilor fantoma sub load.

## P1 - blocante pentru private beta stabil

### Securitate

- Tokenul bearer comun este inclus in `Info.plist` si poate fi extras din orice
  copie a aplicatiei. Este acceptabil numai pentru un beta privat controlat.
- Lipsesc identitatea per instalare, tokenuri scurte, revocare si stocare in
  Keychain.
- Endpoint-urile publice nu au rate limiting, quota sau limite de conexiuni per
  identitate/IP.

Inainte de distributie mai larga: enrollment per instalare, credential in
Keychain, access token scurt si rotabil, plus rate limiting in proxy si server.

### Rezilienta clientului

- Agentul citeste catalogul numai la pornire si nu recupereaza automat dupa
  schimbarea retelei, pierderea WSS sau modificarea catalogului.
- Restartul este fix la 5 secunde, fara exponential backoff si jitter.
- `agent.log` nu are rotatie sau limita de marime.
- Numarul din catalog ramane static si nu reflecta ocuparea live a lobby-ului.

### Release macOS

- Semnarea curenta este ad-hoc; lipsesc Developer ID, hardened runtime,
  notarizare si stapling.
- Lipsesc update-ul semnat si `Launch at Login`.
- Patch-ul WebUI este intentionat version-coupled. Refuzul strict protejeaza
  fisierele necunoscute, dar fiecare build Blizzard nou necesita fixture si
  validare live inainte de allowlist.

## P2 - operare si mentenanta

- lipsesc metrici, dashboard, alerte si runbook operational;
- lipsesc CI, dependency audit, SBOM si scanarea imaginii/containerului;
- imaginea de runtime nu are tagurile de baza fixate prin digest in Dockerfile;
- nu exista persistence pentru identitati, banlist, istoric sau replay;
- nu exista soak/load/chaos test si nici validarea a 50 de jocuri consecutive.

## Gate-uri de release

Un release poate fi numit `private beta` numai dupa:

1. doua Mac-uri joaca 15 minute si obtin acelasi checksum/action stream;
2. disconnect/reconnect, leave in loading si leave in-game au rezultate
   deterministe;
3. credentialele sunt per instalare si revocabile;
4. rate limiting, metrici si alerte sunt active pe Linux;
5. aplicatia este semnata Developer ID si notarizata;
6. cel putin 50 de cicluri join -> map -> start -> game -> cleanup trec fara
   lobby-uri fantoma sau leak-uri.

Un release poate fi numit `production` numai dupa gate-urile de mai sus plus
backup/restore testat, rollout/rollback documentat, SLO-uri si soak test cu
numarul tinta de lobby-uri concurente.
