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

```dotenv
STRAJER_PUBLISH_ADDR=<linux-server-lan-ip>
STRAJER_PORT=18080
STRAJER_RUST_LOG=strajer_server=info,tower_http=info
STRAJER_JOIN_TOKEN=<valoarea-generată>
```

Pornește versiunea nouă și verifică health-ul intern:

```bash
docker compose up -d --build
docker compose ps
curl http://<linux-server-lan-ip>:18080/healthz
curl http://<linux-server-lan-ip>:18080/v1/lobbies
```

Catalogul nou trebuie să raporteze `"max":11`. Endpoint-ul
`/v1/lobbies/synthetic-1/session` este WebSocket și necesită bearer token.

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
