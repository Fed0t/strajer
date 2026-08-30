# Sursele protocolului LAN

Implementarea se bazează pe verificări read-only și pe proiecte publice, fără modificarea Warcraft III.

## Build verificat

Instalarea locală verificată:

```text
Warcraft III 2.0.4.23745
Mach-O x86_64
Bundle: com.blizzard.WarcraftIII
```

Binarul conține cheile LAN `players_num`, `players_max`, `game_secret`, `game_create_time` și `game_data`.

## Format discovery

Sursa principală de interoperabilitate este proiectul [W3Champions FLO](https://github.com/w3champions/flo), publicat sub MIT înainte ca repository-ul GitHub să devină indisponibil. O oglindă read-only a fost folosită pentru verificarea formatului.

Contractul observat:

- service DNS-SD: `_blizzard._udp`;
- subtype Reforged 2.0.x: `_w3xp2774`;
- record suplimentar: DNS RR type `66`;
- payload: protobuf `wc3.GameInfo`;
- `game_data`: structură W3GS codificată ca stat string și Base64;
- TTL folosit de FLO: `4500` secunde.

Pentru framing și host engine vor fi comparate independent:

- [ghostpp-rs](https://github.com/Fatorin/ghostpp-rs), Apache-2.0;
- [GHost++](https://github.com/dcramer/ghostplusplus);
- capturi locale controlate pe build-ul Reforged instalat.

Orice cod reutilizat substanțial dintr-un proiect terț trebuie însoțit de licența și atribuirea corespunzătoare. În milestone-ul curent sunt implementate doar contractele wire necesare interoperabilității.
