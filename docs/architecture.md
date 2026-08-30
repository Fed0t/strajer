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

Control-plane Linux. În primul slice păstrează un lobby sintetic immutable în memorie. Ulterior va deține registry-ul, autentificarea, actorii W3GS și persistența.

### `strajer-agent`

Procesul de networking inclus și supravegheat de `Strajer.app`. Descarcă
lobby-urile, deschide listener-ele locale, publică recordurile LAN și validează
primul `REQJOIN` încadrat de `strajer-w3gs`. Toate conexiunile către server sunt
inițiate outbound.

### `Strajer.app`

Shell SwiftUI `MenuBarExtra`, fără Dock icon. Pornește agentul inclus, consumă
evenimentele JSON de status, îl repornește după failure și afișează starea,
numărul de jocuri și detectarea unui `REQJOIN` valid. Endpoint-ul este inclus în
`Info.plist` la build; utilizatorul nu are configurări.

## Fluxul discovery

1. Agentul cere catalogul `/v1/lobbies`.
2. Pentru fiecare lobby deschide un listener TCP local pe un port dinamic.
3. Construiește `game_data` cu acel port.
4. Publică serviciul `_blizzard._udp` ca Bonjour `LocalOnly`, cu subtype-ul versiunii Warcraft și record DNS type `66`.
5. Warcraft afișează lobby-ul în `Local Area Network`.
6. La `Join`, Warcraft se conectează la listener-ul agentului.

`LocalOnly` este intenționat: fiecare Mac trebuie să vadă numai proxy-ul său
local, chiar dacă mai multe Mac-uri Strajer sunt în același LAN. Serverul Linux
rămâne singurul loc în care sesiunile celor două proxy-uri sunt reunite.

## Evoluția transportului

- Milestone 0: HTTP polling pentru catalog și listener local de diagnostic.
- Milestone 1: control persistent + tunel binar WSS autentificat pe `443/tcp`.
- Milestone 2: actor per lobby și host engine W3GS autoritativ.
- Milestone 3: start, action loop și replay.
- Milestone 4: QUIC pe `443/udp`, WSS fallback și reconectare măsurată.

HTTP polling-ul inițial este intenționat temporar; evită introducerea prematură a unui protocol de control complex înainte ca discovery-ul local să fie validat.

Planul detaliat pentru framing, tunnel și host engine este în
[Plan pentru join W3GS real](join-plan.md).
