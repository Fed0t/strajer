# Arhitectură

```text
Linux
┌───────────────────────────────────────────────────────────┐
│ strajer-server                                            │
│ control API │ lobby registry │ W3GS host │ maps │ replays │
└───────────────────────────┬───────────────────────────────┘
                            │ outbound HTTPS/WSS, apoi QUIC
┌───────────────────────────┴───────────────────────────────┐
│ macOS                                                     │
│ Strajer.app → embedded strajer-agent → DNS-SD publisher   │
│                                       → local TCP listener │
└───────────────────────────────────────┬───────────────────┘
                                        │ local machine only
                               Warcraft III Reforged
```

## Componente

### `strajer-protocol`

Contractele serializabile dintre server și agent. Nu conține detalii Axum, DNS-SD sau UI.

### `strajer-lan`

Codifică recordul LAN Reforged și îl publică prin DNS-SD pe macOS. Portul publicat aparține listener-ului local al agentului, nu serverului Linux.

### `strajer-w3gs`

Implementează framing-ul binar W3GS și decodoarele tipizate, independent de
transport, server și UI. Reader-ul validează signature, lungime și limita
configurată înainte de orice alocare bazată pe input.

### `strajer-server`

Control-plane Linux cu un catalog sintetic, registry concurent de lobby, endpoint
WSS autentificat și endpoint autentificat de map download. La boot validează
dimensiunea, CRC32-ul și SHA-1-ul hărții montate read-only. Alocă player ID-urile
și sloturile umane, rezervă host-ul virtual, publică roster-ul versionat și
curăță sesiunile la disconnect. După ce toate cele 10 sesiuni confirmă harta,
serverul deține countdown-ul de 60 secunde și anulează startul dacă un player
pleacă.
Persistența și identitatea per instalare nu sunt încă implementate.

### `strajer-agent`

Procesul de networking inclus și supravegheat de `Strajer.app`. Descarcă
lobby-urile, deschide listener-ele locale, publică recordurile LAN, validează
`REQJOIN`, deschide sesiunea WSS și traduce roster-ul coordonat în răspunsuri
W3GS locale. Listener-ele Warcraft sunt legate exclusiv pe `127.0.0.1` și `::1`,
pe același port; toate conexiunile către server sunt inițiate outbound.
Hărțile deja instalate sunt reutilizate numai dacă trec validarea manifestului;
altfel agentul descarcă asset-ul într-un cache atomic din
`~/Library/Caches/Strajer/maps` și îl transferă local către Warcraft.

### `Strajer.app`

Shell SwiftUI `MenuBarExtra`, fără Dock icon. Pornește agentul inclus, consumă
evenimentele JSON de status, îl repornește după failure și afișează starea,
numărul de jocuri, nickname-ul persistent și detectarea unui `REQJOIN` valid. Controller-ul de
compatibilitate offline activeaza `Allow Local Files`, detecteaza instalarea
Warcraft prin LaunchServices si deriva override-ul WebUI din serverul loopback al
jocului. Sunt acceptate numai semnaturile cunoscute: cele patru expresii de join
si cele sapte puncte de precompletare a nickname-ului sunt patch-uite, iar un
fisier necunoscut nu este suprascris. Endpoint-ul este inclus
în `Info.plist` la build; utilizatorul nu are configurări.

## Fluxul discovery

1. Agentul cere catalogul `/v1/lobbies`.
2. Pentru fiecare lobby deschide un listener TCP dual-stack IPv4/IPv6 pe un
   singur port dinamic.
3. Construiește `game_data` cu acel port.
4. Publică serviciul `_blizzard._udp` ca Bonjour `LocalOnly`, cu subtype-ul versiunii Warcraft și record DNS type `66`.
5. Warcraft afișează lobby-ul în `Local Area Network`.
6. La `Join`, Warcraft se conectează la listener-ul agentului.
7. Agentul autentifică o sesiune WSS la server, primește player ID-ul și roster-ul.
8. Agentul răspunde local cu `SLOTINFOJOIN`, `PLAYERINFO`, profile/skins și
   `MAPCHECK`, apoi aplică update-urile de roster în lobby-ul Warcraft.
9. Dacă Warcraft raportează harta lipsă, agentul trimite `STARTDOWNLOAD` și
   `MAPPART` în ferestre de maximum 100 × 1.442 bytes, avansate de confirmările
   `MAPSIZE`; fiecare fragment are CRC32 propriu.
10. Agentul confirma `ready` numai dupa verificarea hartii. La 10/10 jucatori
    ready, serverul publica 60/50/40/30/20/10 in chat prin host-ul virtual
    `Strajer`, apoi trimite tuturor tranzitia W3GS spre loading.

`LocalOnly` este intenționat: fiecare Mac trebuie să vadă numai proxy-ul său
local, chiar dacă mai multe Mac-uri Strajer sunt în același LAN. Serverul Linux
rămâne singurul loc în care sesiunile celor două proxy-uri sunt reunite.

## Evoluția transportului

- Milestone 0: HTTP polling pentru catalog și listener local de diagnostic.
- Milestone 1 curent: control persistent WSS autentificat pe `443/tcp`.
- Milestone 2 curent: registry de lobby server-side și proiecție W3GS locală.
- Milestone 2.1 curent: distribuție HTTPS, cache verificat și map transfer W3GS.
- Milestone 3 curent: host virtual, ready, countdown si start spre loading.
- Milestone 3.1: load sync uman, action loop, leave/lag si replay.
- Milestone 4: QUIC pe `443/udp`, WSS fallback și reconectare măsurată.

HTTP polling-ul inițial este intenționat temporar; evită introducerea prematură a unui protocol de control complex înainte ca discovery-ul local să fie validat.

Planul detaliat pentru framing, tunnel și host engine este în
[Plan pentru join W3GS real](join-plan.md).
