# Plan de dezvoltare Strajer

## Obiectiv

Experiența finală trebuie să fie zero-configuration după prima pornire:

1. utilizatorul pornește `Strajer.app` pe macOS;
2. aplicația rămâne în menu bar și se conectează outbound la serverul Strajer;
3. utilizatorul pornește Warcraft III normal;
4. lobby-urile Strajer apar în `Local Area Network`;
5. jucători aflați în rețele diferite intră în același joc găzduit pe Linux.

Serverul este operat centralizat și nu există configurare de endpoint în aplicația finală.

## Principii

- Fără modificarea binarului sau arhivei CASC Warcraft III. Orice override WebUI
  local trebuie derivat din instalarea utilizatorului, verificat strict și
  refuzat pentru versiuni necunoscute.
- Fără integrare cu matchmaking-ul sau catalogul Multiplayer Battle.net.
- Protocolul LAN, control-plane-ul și transportul jocului rămân module separate.
- Serverul acceptă numai conexiuni autentificate și criptate înainte de expunerea publică.
- Fiecare milestone are un test end-to-end observabil, nu doar cod compilabil.
- PostgreSQL, caching și orchestrare multi-node se introduc numai când starea persistentă sau măsurătorile le justifică.

## Milestone 0 — Fezabilitate și fundație

Durată estimată: 2–4 zile.

Livrabile:

- workspace Rust și contracte comune;
- server HTTP cu `/healthz`, `/readyz` și `/v1/lobbies`;
- imagine Docker non-root și read-only;
- publisher DNS-SD macOS `LocalOnly` pentru recordul Reforged type `66`;
- listener TCP local pentru primul `REQJOIN`;
- teste unitare pentru service type, protobuf și `game_data`.

Criteriu de ieșire:

- `Strajer Test #1` apare pe două Mac-uri care rulează agentul și consultă același server.

Status la 30 august 2026: discovery-ul este validat pe primul Mac, iar cele două
aplicații se pot conecta simultan la endpoint-ul public. Coliziunea Bonjour
observată când ambele Mac-uri erau în același LAN a fost corectată prin publicare
`LocalOnly` și verificată la runtime pe primul Mac. Revalidarea pachetului nou pe
ambele Mac-uri în același LAN rămâne criteriul final de închidere.

## Milestone 1 — Join remote end-to-end

Durată estimată: 1–2 săptămâni.

Livrabile:

- identitate per instalare și token de sesiune scurt;
- canal control persistent;
- tunel binar agent–server cu backpressure și timeout-uri;
- rutare `local game id -> server lobby id`;
- parser framing W3GS și validarea `REQJOIN`;
- metrici pentru connect, reject, timeout și RTT.

Criteriu de ieșire:

- două Mac-uri trimit `REQJOIN` valid aceluiași lobby Linux, fără port forwarding pe clienți.

Status la 31 august 2026: framing-ul incremental și parserul tolerant `REQJOIN`
sunt integrate în agent, descriptorul DotA real este verificat, iar canalul WSS
autentificat sincronizează două sesiuni de agent prin server. Token-ul comun
inclus în bundle este doar hardening pentru private beta; identitatea per
instalare, token-urile scurte, metricile și data-plane-ul de gameplay rămân
deschise.

Specificația tehnică, deciziile de transport și criteriile intermediare J0–J4
sunt detaliate în [Plan pentru join W3GS real](join-plan.md).

## Milestone 2 — Lobby W3GS real

Durată estimată: 2–4 săptămâni.

Livrabile:

- actor per lobby și actor per player;
- slot management, profile și skins Reforged;
- `SLOTINFOJOIN`, `PLAYERINFO`, `MAPCHECK` și map availability;
- endpoint de hartă autentificat, cache local atomic și map transfer W3GS cu
  sliding window;
- propagarea player count-ului în recordurile LAN;
- disconnect și cleanup determinist.

Criteriu de ieșire:

- doi jucători apar reciproc în lobby, iar sloturile rămân sincronizate după reconnect.

Status la 31 august 2026: serverul alocă determinist player ID și slot, iar
agentul emite `SLOTINFOJOIN`, `PLAYERINFO`, profile/skins Reforged, `MAPCHECK`,
update de roster și cleanup la leave. Serverul validează harta montată read-only,
iar agentul implementează download HTTPS autentificat, cache SHA-1/CRC32 și
transfer W3GS `STARTDOWNLOAD`/`MAPPART`. Codec-urile și pipeline-ul HTTP sunt
validate automat; fluxul a fost validat local în UI numai cu harta deja
instalată. Testul live pe un Mac fără hartă și validarea pe două Mac-uri rămân
criterii deschise.

## Milestone 3 — Joc complet

Durată estimată: 4–8 săptămâni.

Livrabile:

- host virtual `Strajer` in slotul `HOSTBOT` si 10 sloturi umane;
- chat lobby bidirectional intre toate sesiunile coordonate;
- ready per client dupa verificarea hartii, countdown automat de 60 secunde si
  start autoritativ pe server cand toate sloturile umane sunt ocupate;
- fallback `!start` pentru minimum doi jucatori conectati, activ numai dupa ce
  toate sesiunile curente sunt ready;
- load synchronization pentru jucatorii umani;
- action batching, keepalive și desync detection;
- leave/lag handling;
- generare și validare replay `.w3g`.

Criteriu de ieșire:

- două instanțe Reforged joacă minimum 15 minute și produc un replay valid, repetat în 50 de cicluri consecutive.

Status la 31 august 2026: host-ul virtual, chatul lobby, ready, countdown-ul
60/50/40/30/20/10, fallback-ul `!start`, anularea la leave si pachetele
`COUNTDOWN_START`/`COUNTDOWN_END`, `GAMELOADED_SELF` si propagarea
`GAMELOADED_OTHERS` sunt implementate. Action batching, timeslot-urile,
leave/lag in-game si replay raman blockerele pentru gameplay complet.

## Milestone 4 — Strajer.app

Durată estimată: 2–3 săptămâni, parțial în paralel cu milestone-urile 1–3.

Livrabile:

- shell SwiftUI `MenuBarExtra` fără Dock icon;
- agent Rust inclus și supravegheat de aplicație;
- status `Connected`, `Connecting`, `Unavailable` și numărul de jocuri;
- permisiune Local Network explicată corect;
- launch at login, update semnat, crash recovery;
- build universal, code signing și notarizare.

Criteriu de ieșire:

- după instalare și acordarea permisiunii Local Network nu există configurare manuală.

Status la 31 august 2026: shell-ul `MenuBarExtra`, agentul inclus, statusul,
indicatorul `Join request detected` și build-ul universal sunt implementate.
Aplicatia activeaza automat workaround-ul WebUI offline, derivat si validat din
instalarea Warcraft locala, fara a redistribui fisierul Blizzard. Nickname-ul se
salveaza dupa primul join si poate fi schimbat din actiunea `Nickname...` din
menu bar.
Launch at login, update-ul semnat, crash reporting, Developer ID și notarizarea
rămân deschise.

## Milestone 5 — Producție Linux

Durată estimată: 2–4 săptămâni.

Livrabile:

- TLS public, key pinning și rotație controlată;
- rate limiting, quotas și protecție la abuse;
- PostgreSQL pentru identități, banlist și istoric;
- backup, restore și politici de retenție replay;
- metrics, structured logs, alerting și runbook;
- deploy Docker reproductibil cu rollback.

Criteriu de ieșire:

- serviciul supraviețuiește restarturilor, update-urilor și pierderilor temporare de rețea fără lobby-uri fantomă sau coruperea sesiunilor.

## Riscuri urmărite explicit

- schimbări de protocol între build-urile Reforged;
- diferențe macOS/Windows în Bonjour și în pachetele W3GS;
- map hash/checksum și accesul la CASC;
- WSS/TCP folosit prea mult după proof-of-concept, head-of-line blocking și reconnect;
- listener local expus pe interfața LAN înainte de verificarea bind-ului loopback;
- limite EULA și autorizare pentru distribuție publică;
- semnare/notarizare și Local Network privacy pe macOS.

Orice deploy pe serverul Linux real necesită separat detaliile infrastructurii și cele două aprobări explicite pentru conexiunea SSH.
