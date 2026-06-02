# Bases de données

PaaSTech permet de provisionner des instances **PostgreSQL** managées en tant que ressources. Chaque ressource est un conteneur Docker isolé sur le réseau `paas-net`, géré par la plateforme.

Les autres types de ressources disponibles sont Redis (cache) et Garage (stockage objet S3) — consultez la [Référence API](../api/reference.md#ressources) pour leur usage similaire.

---

## Provisionner une instance PostgreSQL

```
POST /resource
```

### Corps de la requête

```json
{
  "display_name": "Base de production",
  "name": "postgres",
  "version": "16",
  "application_id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890"
}
```

| Champ | Type | Description | Obligatoire |
|---|---|---|---|
| `display_name` | `string` | Nom affiché de la ressource | Oui |
| `name` | `string` | Type de service : `postgres`, `redis`, `s3` | Oui |
| `version` | `string` | Tag Docker du service (ex : `16`, `15`, `alpine`) | Oui |
| `application_id` | `string` (UUID) | ID de l'application à associer | Non |

### Exemple

```bash
curl -X POST http://localhost:8080/resource \
  -H "Content-Type: application/json" \
  -d '{
    "display_name": "Ma PostgreSQL",
    "name": "postgres",
    "version": "16"
  }'
```

### Réponse (`201 Created`)

```json
{
  "id": "b2c3d4e5-f6a7-8901-bcde-f12345678901",
  "display_name": "Ma PostgreSQL",
  "name": "postgres",
  "version": "16",
  "status": "running",
  "application_ids": []
}
```

La ressource est **immédiatement démarrée** après sa création. Un port hôte lui est alloué dynamiquement. La validation du tag Docker sur Docker Hub est effectuée avant création.

---

## Versions disponibles

```bash
# Lister les versions disponibles pour PostgreSQL
curl http://localhost:8080/service/postgres/versions
```

```json
["16", "15", "14", "13", "alpine", "16-alpine", ...]
```

---

## Variables d'environnement injectées automatiquement

Lors de la création, PaaSTech génère et stocke les variables d'environnement suivantes pour PostgreSQL :

| Variable | Description | Exemple |
|---|---|---|
| `POSTGRES_USER` | Utilisateur de la base | `paastech` |
| `POSTGRES_PASSWORD` | Mot de passe (généré) | `xK9mP2...` |
| `POSTGRES_DB` | Nom de la base de données | `paastech` |

Ces variables sont injectées dans le conteneur PostgreSQL au démarrage. Pour les consulter :

```bash
curl http://localhost:8080/resource/{id}/env
```

---

## Attacher une base de données à une application

Une ressource peut être associée à une application lors de sa création via le champ `application_id`, ou après coup via `PATCH /resource/{id}`.

### À la création

```bash
curl -X POST http://localhost:8080/resource \
  -H "Content-Type: application/json" \
  -d '{
    "display_name": "BDD de mon-app",
    "name": "postgres",
    "version": "16",
    "application_id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890"
  }'
```

### Après création (mise à jour)

```bash
curl -X PATCH http://localhost:8080/resource/{resource_id} \
  -H "Content-Type: application/json" \
  -d '{
    "application_ids": ["a1b2c3d4-e5f6-7890-abcd-ef1234567890"]
  }'
```

L'association est stockée dans la table `application_services`. Le champ `application_ids` du `PATCH` remplace toutes les associations existantes.

---

## Arrêter et démarrer une instance

```bash
# Arrêter
curl -X POST http://localhost:8080/resource/{id}/stop

# Démarrer
curl -X POST http://localhost:8080/resource/{id}/start
```

Tenter d'arrêter une ressource déjà arrêtée, ou de démarrer une ressource déjà démarrée, retourne `409 Conflict`.

---

## Supprimer une instance

```
DELETE /resource/{id}
```

Supprime la ressource de la base de données. La suppression en cascade supprime les associations `application_services` et les variables d'environnement `service_env_vars`.

```bash
# Arrêter le conteneur avant suppression
curl -X POST http://localhost:8080/resource/{id}/stop
curl -X DELETE http://localhost:8080/resource/{id}
```

Réponse : `204 No Content`.

---

## Exemple complet end-to-end

```bash
# 1. Vérifier les versions disponibles
curl http://localhost:8080/service/postgres/versions

# 2. Créer la ressource PostgreSQL
RESOURCE=$(curl -s -X POST http://localhost:8080/resource \
  -H "Content-Type: application/json" \
  -d '{"display_name": "PostgreSQL prod", "name": "postgres", "version": "16"}')

RESOURCE_ID=$(echo $RESOURCE | python3 -c "import sys,json; print(json.load(sys.stdin)['id'])")
echo "ID: $RESOURCE_ID"

# 3. Récupérer les credentials
curl http://localhost:8080/resource/$RESOURCE_ID/env
# => {"POSTGRES_USER": "paastech", "POSTGRES_PASSWORD": "...", "POSTGRES_DB": "paastech"}

# 4. Récupérer le port hôte
PORT=$(curl -s http://localhost:8080/resource/$RESOURCE_ID \
  | python3 -c "import sys,json; print(json.load(sys.stdin).get('port',''))")

# 5. Se connecter à la base depuis l'hôte
psql postgresql://paastech:<password>@localhost:$PORT/paastech

# 6. Arrêter puis supprimer
curl -X POST http://localhost:8080/resource/$RESOURCE_ID/stop
curl -X DELETE http://localhost:8080/resource/$RESOURCE_ID
```
