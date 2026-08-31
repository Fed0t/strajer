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

Pentru descriptorul hărții, FLO confirmă separarea dintre SHA-1-ul și CRC32-ul
brut al arhivei MPQ și checksum-ul Xoro calculat peste scriptul și fișierele
interne relevante. Harta DotA locală a fost citită read-only cu StormLib, care
suportă compresia PKWare DCL folosită de arhivă.

Pentru framing și host engine vor fi comparate independent:

- [GHost++ `gameprotocol.cpp`](https://github.com/dcramer/ghostplusplus/blob/master/ghost/gameprotocol.cpp),
  inclusiv layout-ul clasic `W3GS_REQJOIN`, generarea `SLOTINFOJOIN`,
  `STARTDOWNLOAD` și `MAPPART`;
- [GHost++ `game_base.cpp`](https://github.com/dcramer/ghostplusplus/blob/master/ghost/game_base.cpp),
  pentru semantica `MAPSIZE` și fereastra de 100 fragmente × 1.442 bytes;
- [ghostpp-rs](https://github.com/Fatorin/ghostpp-rs), Apache-2.0, ca referință
  pentru framing și modelul actor;
- [gowarcraft3 W3GS client](https://github.com/nielsAD/gowarcraft3/blob/master/cmd/w3gsclient/main.go),
  ca implementare independentă;
- [Wireshark BNETP/W3GS dissector](https://github.com/diegonc/packet-bnetp/blob/main/packet-bnetp-w3gs.lua),
  pentru verificarea packet ID-urilor;
- capturi locale controlate pe build-ul Reforged instalat.

Pentru action loop-ul din session protocol `5`, implementarile clasice au fost
verificate incrucisat astfel:

- [gowarcraft3 `protocol/w3gs/packets.go`](https://github.com/nielsAD/gowarcraft3/blob/master/protocol/w3gs/packets.go)
  confirma layout-ul client-side pentru `OUTGOING_ACTION` (`0x26`: CRC32 urmat
  de action bytes) si `OUTGOING_KEEPALIVE` (`0x27`: byte necunoscut plus
  checksum `u32`);
- [GHost++ `gameprotocol.cpp`](https://github.com/dcramer/ghostplusplus/blob/master/ghost/gameprotocol.cpp)
  confirma `INCOMING_ACTION` (`0x0C`), CRC-ul low-16, recordurile per player si
  fragmentele `INCOMING_ACTION2` (`0x48`) trimise inaintea frame-ului final;
- [GHost++ `game_base.cpp`](https://github.com/dcramer/ghostplusplus/blob/master/ghost/game_base.cpp)
  confirma tick-ul clasic de 100 ms, limita de 1.452 bytes pentru subpacket si
  compararea checksum-urilor numai dupa ce fiecare player activ are o valoare.
  `EventPlayerLoaded` publica `GAMELOADED_OTHERS` tuturor sesiunilor, inclusiv
  sesiunii care a trimis `GAMELOADED_SELF`; Warcraft asteapta confirmarea
  coordonata pentru fiecare PID inainte de primul timeslot.

Implementările publice clasice nu sunt tratate drept dovadă că Reforged păstrează
identic fiecare câmp. Fixture-urile build-ului local au prioritate când există o
diferență observată.

Orice cod reutilizat substanțial dintr-un proiect terț trebuie însoțit de licența și atribuirea corespunzătoare. În milestone-ul curent sunt implementate doar contractele wire necesare interoperabilității.
