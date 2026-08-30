# Arhitectură

```text
Linux
┌───────────────────────────────────────────────────────────┐
│ strajer-server                                            │
│ control API │ lobby registry │ W3GS host │ maps │ replays │
└───────────────────────────┬───────────────────────────────┘
                            │ outbound TLS/QUIC
┌───────────────────────────┴───────────────────────────────┐
│ macOS                                                     │
│ Strajer.app → embedded strajer-agent → DNS-SD publisher   │
│                                       → local TCP listener │
└───────────────────────────────────────┬───────────────────┘
                                        │ localhost/LAN view
                               Warcraft III Reforged
```

## Componente

### `strajer-protocol`

Contractele serializabile dintre server și agent. Nu conține detalii Axum, DNS-SD sau UI.

### `strajer-lan`

Codifică recordul LAN Reforged și îl publică prin DNS-SD pe macOS. Portul publicat aparține listener-ului local al agentului, nu serverului Linux.

### `strajer-server`

Control-plane Linux. În primul slice păstrează un lobby sintetic immutable în memorie. Ulterior va deține registry-ul, autentificarea, actorii W3GS și persistența.

### `strajer-agent`

Procesul de networking inclus și supravegheat de `Strajer.app`. Descarcă lobby-urile, deschide listener-ele locale și publică recordurile LAN. Toate conexiunile către server sunt inițiate outbound.

### `Strajer.app`

Shell SwiftUI `MenuBarExtra`, fără Dock icon. Pornește agentul inclus, consumă evenimentele JSON de status, îl repornește după failure și afișează starea plus numărul de jocuri. Endpoint-ul este inclus în `Info.plist` la build; utilizatorul nu are configurări.

## Fluxul discovery

1. Agentul cere catalogul `/v1/lobbies`.
2. Pentru fiecare lobby deschide un listener TCP local pe un port dinamic.
3. Construiește `game_data` cu acel port.
4. Publică serviciul `_blizzard._udp` cu subtype-ul versiunii Warcraft și record DNS type `66`.
5. Warcraft afișează lobby-ul în `Local Area Network`.
6. La `Join`, Warcraft se conectează la listener-ul agentului.

## Evoluția transportului

- Milestone 0: HTTP polling pentru catalog și listener local de diagnostic.
- Milestone 1: control persistent + tunel binar autentificat.
- Milestone 3: transport orientat pe mesaje W3GS, cu fallback și reconectare măsurată.

HTTP polling-ul inițial este intenționat temporar; evită introducerea prematură a unui protocol de control complex înainte ca discovery-ul local să fie validat.
