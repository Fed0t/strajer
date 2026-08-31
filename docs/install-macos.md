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
5. Iconița shield apare în menu bar; meniul trebuie să afișeze `Connected` și numărul de jocuri.
6. Pornește Warcraft III normal și intră în `Local Area Network`.
7. Ambii jucători aleg `Strajer Test #1`; după join trebuie să apară `2/11` și
   ambele nume în sloturi.

Build-ul local este semnat ad-hoc. Distribuția fără avertisment Gatekeeper necesită certificat Apple Developer ID, hardened runtime și notarizare Apple.
