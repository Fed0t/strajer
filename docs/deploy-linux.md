# Deploy Linux și Nginx Proxy Manager

## Container

În directorul repository-ului de pe server:

```bash
cp .env.example .env
openssl rand -hex 32
```

Copiază rezultatul o singură dată în `STRAJER_JOIN_TOKEN` din `.env`. Nu adăuga
`.env` în Git și păstrează aceeași valoare pentru build-urile macOS. Pentru
serverul actual, configurația este:

Înainte de deploy, pune harta exactă în `maps/DotA_v6_89Q.w3x`. Fișierul nu este
inclus în imaginea Docker sau în Git; Compose montează directorul read-only în
container. Serverul refuză să pornească dacă dimensiunea, CRC32-ul sau SHA-1-ul
nu corespund manifestului.

```dotenv
STRAJER_PUBLISH_ADDR=<linux-server-lan-ip>
STRAJER_PORT=18080
STRAJER_RUST_LOG=strajer_server=info,tower_http=info
STRAJER_MAPS_DIR=./maps
STRAJER_JOIN_TOKEN=<valoarea-generată>
```

Pornește versiunea nouă și verifică health-ul intern:

```bash
docker compose up -d --build
docker compose ps
curl http://<linux-server-lan-ip>:18080/healthz
curl http://<linux-server-lan-ip>:18080/v1/lobbies
```

Catalogul nou trebuie să raporteze `"current":1`, `"max":11` si
`"virtual_host":{"player_id":11,"slot_index":10,"name":"Strajer"}`. Endpoint-ul
`/v1/lobbies/synthetic-1/session` este WebSocket și necesită bearer token.
Catalogul schema `3` si protocolul de sesiune `3` introduc host-ul virtual,
chatul bidirectional, `ready`, countdown si `!start`; deploy-ul serverului si
build-urile noi de client trebuie coordonate. Un client cu session protocol `2`
este refuzat intentionat de serverul nou.

## Nginx Proxy Manager

Proxy Host-ul `strajer.clarixpro.com` trebuie să aibă:

- Scheme: `http`;
- Forward Hostname/IP: adresa LAN a serverului Linux;
- Forward Port: `18080`;
- `Websockets Support`: activ;
- certificat Let's Encrypt și `Force SSL`: active.

Nu este necesar un port Strajer suplimentar în router. HTTPS și WSS folosesc
același `443/tcp` terminat de Nginx Proxy Manager; portul `18080` rămâne numai în
LAN.

Verificarea publică după deploy:

```bash
curl https://strajer.clarixpro.com/healthz
curl https://strajer.clarixpro.com/v1/lobbies
curl -f -H "Authorization: Bearer ${STRAJER_JOIN_TOKEN}" \
  -o /dev/null \
  https://strajer.clarixpro.com/v1/maps/c771ac8d7dc3665a211c2b1432672d49bfba1bcf
```

## Build pentru ambele Mac-uri

Pe Mac-ul de build, introdu token-ul serverului fără echo:

```bash
read -s "STRAJER_JOIN_TOKEN?Strajer join token: "
export STRAJER_JOIN_TOKEN
STRAJER_SERVER_URL=https://strajer.clarixpro.com scripts/build-macos-app.sh
scripts/package-macos-app.sh
unset STRAJER_JOIN_TOKEN
```

Transferă `dist/Strajer-0.1.0-macos-universal.zip` pe celălalt Mac. Ambele
instalări trebuie să provină din acest build; altfel vor folosi endpoint-uri sau
token-uri diferite.
