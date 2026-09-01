# Runbook Private Beta Strajer

## Scop

Acest runbook acopera deploy-ul coordonat server + client, probele de sanatate,
testul pe doua Mac-uri si criteriile de oprire. Nu acopera session resume in
mijlocul gameplay-ului, semnarea Developer ID sau notarizarea.

Seteaza endpoint-ul public o singura data in shell-ul operational:

```bash
export STRAJER_SERVER_URL="https://strajer.clarixpro.com"
```

## Gate inainte de deploy

Ruleaza local, fara a incarca `.env` in Git:

```bash
cargo fmt --all -- --check
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
docker compose build strajer-server
```

Pastreaza artefactul server anterior si ZIP-ul macOS anterior pentru rollback.
Serverul si ambele aplicatii trebuie construite din acelasi commit si trebuie sa
foloseasca acelasi session protocol si acelasi token private-beta.

## Deploy Linux

Pe server, dupa sincronizarea sursei si a hartii verificate:

```bash
docker compose up -d --build strajer-server
docker compose ps
docker compose logs --tail=200 strajer-server
```

Deploy-ul este acceptat numai daca containerul este `healthy`, iar probele
publice raspund cu HTTP 200:

```bash
curl --fail --silent --show-error "${STRAJER_SERVER_URL}/healthz"
curl --fail --silent --show-error "${STRAJER_SERVER_URL}/readyz"
curl --fail --silent --show-error "${STRAJER_SERVER_URL}/v1/lobbies"
```

`/readyz` verifica inclusiv asset-ul hartii. Nginx Proxy Manager trebuie sa aiba
WebSocket Support activ; HTTPS si WSS folosesc acelasi port public `443/tcp`.

## Build si distributie macOS

Introdu token-ul fara echo, construieste universal si verifica checksum-ul:

```bash
read -s "STRAJER_JOIN_TOKEN?Strajer join token: "
export STRAJER_JOIN_TOKEN
STRAJER_SERVER_URL="${STRAJER_SERVER_URL}" scripts/build-macos-app.sh
scripts/package-macos-app.sh
unset STRAJER_JOIN_TOKEN
pushd dist
shasum -a 256 -c Strajer-0.1.0-macos-universal.zip.sha256
popd
```

Ambele Mac-uri trebuie sa foloseasca exact acelasi ZIP verificat.

## Matrice minima pe doua Mac-uri

1. Porneste ambele aplicatii; fiecare trebuie sa arate `Connected` si un singur
   lobby in `Local Area Network`.
2. Intra de pe ambele Mac-uri. Fiecare trebuie sa vada cele doua nickname-uri si
   host-ul virtual `Strajer`.
3. Trimite cate un mesaj de pe fiecare Mac. Mesajul trebuie sa apara exact o data
   pe ambele ecrane, inclusiv la expeditor.
4. Trimite `!start`; verifica acelasi countdown si aceeasi tranzitie loading pe
   ambele instante.
5. Joaca minimum 15 minute si confirma ca action stream-ul ramane sincron.
6. Da leave pe un Mac in loading si apoi intr-un test separat in gameplay;
   celalalt Mac trebuie sa primeasca leave-ul si inchiderea determinista.
7. Intrerupe temporar conexiunea serverului intr-un test controlat. Clientul
   trebuie sa curete `Lobby joined` in aproximativ 35 secunde, iar serverul nu
   trebuie sa pastreze slotul mai mult de aproximativ 45 secunde. Rejoin-ul din
   lista LAN trebuie sa functioneze fara restart Strajer.
8. Schimba reteaua unui Mac. Agentul trebuie sa treaca prin `Reconnecting` si sa
   republice lobby-ul dupa revenirea conectivitatii.

Pentru primul beta executa minimum 10 cicluri complete. Gate-ul de release ramane
50 de cicluri consecutive fara sloturi fantoma, task-uri blocate sau desync.

## Diagnostic

Logurile locale sunt bounded si se afla in:

```text
~/Library/Logs/Strajer/agent.log
~/Library/Logs/Strajer/agent.log.1
~/Library/Logs/Strajer/agent.log.2
~/Library/Logs/Strajer/agent.log.3
```

Evenimentele `join_request_captured`, `lobby_joined` si
`lobby_session_ended` trebuie corelate prin acelasi `connection_id`. Pe server,
coreleaza `lobby_id`, `player_id` si `session_id`; nu copia tokenul in loguri sau
rapoarte.

## Stop si rollback

Opreste rollout-ul daca `/readyz` nu este 200, schema/protocolul difera intre
client si server, apar mesaje chat duplicate, sloturi fantoma, desync sau crash
loop. Redeployeaza artefactul server anterior si redistribuie ZIP-ul pereche
anterior; nu combina un client nou cu un protocol server vechi.
