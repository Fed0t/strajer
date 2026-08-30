# Instalare Strajer pe macOS

## Build distributabil

Endpoint-ul serverului este inclus în aplicație; utilizatorul nu configurează nimic:

```bash
STRAJER_SERVER_URL=https://strajer.example.com \
  scripts/build-macos-app.sh

scripts/package-macos-app.sh
```

Rezultatul este `dist/Strajer-0.1.0-macos-universal.zip`, compatibil cu Apple Silicon și Intel.

Nu copia build-ul implicit bazat pe `127.0.0.1`: pe al doilea Mac ar afișa `Unavailable`. Pentru utilizare între rețele, reconstruiește aplicația cu URL-ul HTTPS public al containerului Linux.

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

Build-ul local este semnat ad-hoc. Distribuția fără avertisment Gatekeeper necesită certificat Apple Developer ID, hardened runtime și notarizare Apple.
