# Instalare Strajer pe macOS

## Build distributabil

Endpoint-ul serverului și token-ul private-beta sunt incluse în aplicație;
utilizatorul final nu configurează nimic. Pe Mac-ul de build, citește token-ul
fără echo și folosește exact valoarea din `.env` de pe server:

```bash
read -s "STRAJER_JOIN_TOKEN?Strajer join token: "
export STRAJER_JOIN_TOKEN
STRAJER_SERVER_URL=https://strajer.clarixpro.com scripts/build-macos-app.sh
unset STRAJER_JOIN_TOKEN

scripts/package-macos-app.sh
```

Rezultatul este `dist/Strajer-0.1.0-macos-universal.zip`, compatibil cu Apple Silicon și Intel.

Nu copia build-ul implicit bazat pe `127.0.0.1`: pe al doilea Mac ar afișa
`Unavailable`. Un build public fără token este refuzat de script. Token-ul este
stocat în `Info.plist`, deci poate fi extras de un utilizator local; această
protecție este potrivită numai pentru private beta, nu înlocuiește identitatea per
instalare și Keychain.

Validează checksum-ul înainte de copiere:

```bash
cd dist
shasum -a 256 -c Strajer-0.1.0-macos-universal.zip.sha256
```

## Primul start pe al doilea Mac

1. Copiază ZIP-ul pe Mac și dezarhivează-l.
2. Mută `Strajer.app` în `Applications`.
3. La prima pornire folosește `Right click → Open`, apoi confirmă `Open`.
4. Acceptă permisiunea `Local Network` dacă macOS o cere.
5. Iconița shield apare în menu bar; meniul trebuie să afișeze `Connected` si
   `Offline LAN fix: Ready` sau `Offline LAN fix: Start Warcraft once`.
6. Optional, seteaza numele din `Nickname...`. Daca il completezi prima data in
   dialogul Warcraft, Strajer il salveaza automat pentru join-urile urmatoare.
7. Daca meniul cere pornirea Warcraft, porneste jocul o data. Strajer citeste
   `GlueManager.js` direct din instanta Warcraft locala, valideaza semnatura
   versiunii si instaleaza automat override-ul compatibil.
8. Cand meniul afiseaza `Offline LAN fix: Restart Warcraft once`, inchide complet
   Warcraft si porneste-l din nou. Acest restart este necesar numai dupa prima
   instalare a fixului.
9. Verifica `Offline LAN fix: Ready`, apoi intra in `Local Area Network`.
10. Ambii jucători aleg `Strajer Test #1`; după join trebuie să apară `3/11`:
    cele doua nume umane plus `Strajer` in slotul `HOSTBOT`.

Strajer activeaza automat preferinta Warcraft `Allow Local Files`. Bundle-ul nu
contine si nu redistribuie fisiere Blizzard: patch-ul este derivat din versiunea
Warcraft instalata pe acelasi Mac si schimba numai cele patru apeluri offline
afectate plus cele sapte valori initiale de nickname. Arhiva CASC originala
ramane nemodificata. Daca semnatura WebUI nu este
recunoscuta, Strajer nu suprascrie fisierul si afiseaza `Unavailable`.

Pentru rollback, inchide ambele aplicatii, sterge numai override-ul
`_retail_/webui/GlueManager.js` si fisierul
`_retail_/webui/strajer-config.json`, apoi elimina preferinta cu:

```bash
defaults delete 'com.blizzard.Warcraft III' 'Allow Local Files'
```

Harta nu trebuie copiată manual pe al doilea Mac. Agentul verifică mai întâi o
copie locală existentă, apoi cache-ul Strajer, iar la nevoie descarcă asset-ul
autentificat de pe server. Warcraft primește harta prin W3GS și o salvează în
directorul său normal `Maps/Download`. Pentru harta curentă sunt necesari circa
35 MB liberi atât în cache-ul Strajer, cât și în directorul Warcraft.

Build-ul local este semnat ad-hoc. Distribuția fără avertisment Gatekeeper necesită certificat Apple Developer ID, hardened runtime și notarizare Apple.
