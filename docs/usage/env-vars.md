# Variables d'environnement

Les variables d'environnement dans PaaSTech s'appliquent aux **ressources** (services managés comme PostgreSQL, Redis, S3). Elles sont stockées en base dans la table `service_env_vars` et injectées dans le conteneur lors du démarrage de la ressource.

---

## Lire les variables d'environnement

```
GET /resource/{id}/env
```

Retourne un objet JSON avec toutes les variables de la ressource.

### Exemple

```bash
curl http://localhost:8080/resource/a1b2c3d4-e5f6-7890-abcd-ef1234567890/env
```

### Réponse

```json
{
  "POSTGRES_USER": "paastech",
  "POSTGRES_PASSWORD": "un_mot_de_passe_généré",
  "POSTGRES_DB": "paastech"
}
```

---

## Modifier les variables d'environnement

```
PUT /resource/{id}/env
```

Remplace **toutes** les variables d'environnement existantes par celles fournies dans le corps. C'est une opération de remplacement complet (pas un merge).

### Corps de la requête

```json
{
  "POSTGRES_USER": "mon_user",
  "POSTGRES_PASSWORD": "mon_mot_de_passe_securise",
  "POSTGRES_DB": "ma_base"
}
```

### Exemple

```bash
curl -X PUT http://localhost:8080/resource/a1b2c3d4-e5f6-7890-abcd-ef1234567890/env \
  -H "Content-Type: application/json" \
  -d '{
    "POSTGRES_USER": "mon_user",
    "POSTGRES_PASSWORD": "mon_mot_de_passe_securise",
    "POSTGRES_DB": "ma_base"
  }'
```

Réponse : `200 OK` avec le message `Environment variables updated. Restart the resource to apply changes.`

Les variables mises à jour ne sont prises en compte qu'au **prochain démarrage** du conteneur. Pour appliquer les changements immédiatement :

```bash
# 1. Arrêter la ressource
curl -X POST http://localhost:8080/resource/{id}/stop
# 2. Redémarrer la ressource
curl -X POST http://localhost:8080/resource/{id}/start
```

---

## Variables par défaut des ressources

Lors de la création d'une ressource, des variables d'environnement par défaut sont générées automatiquement selon le type de service.

### PostgreSQL

| Variable | Description | Valeur par défaut |
|---|---|---|
| `POSTGRES_USER` | Nom d'utilisateur | `paastech` |
| `POSTGRES_PASSWORD` | Mot de passe | Généré aléatoirement |
| `POSTGRES_DB` | Nom de la base | `paastech` |

### Redis

| Variable | Description | Valeur par défaut |
|---|---|---|
| `REDIS_PASSWORD` | Mot de passe Redis | Généré aléatoirement |

### Garage S3

| Variable | Description | Valeur par défaut |
|---|---|---|
| `GARAGE_RPC_SECRET` | Secret RPC Garage | Généré aléatoirement |

Les valeurs générées aléatoirement sont créées au moment de la création de la ressource. Elles peuvent être consultées via `GET /resource/{id}/env` et modifiées via `PUT /resource/{id}/env`.

---

## Exemple complet

```bash
# 1. Créer une ressource PostgreSQL
RESOURCE_ID=$(curl -s -X POST http://localhost:8080/resource \
  -H "Content-Type: application/json" \
  -d '{"display_name": "Ma BDD", "name": "postgres", "version": "16"}' \
  | python3 -c "import sys,json; print(json.load(sys.stdin)['id'])")

echo "Resource ID: $RESOURCE_ID"

# 2. Lire les variables générées
curl http://localhost:8080/resource/$RESOURCE_ID/env

# 3. Modifier le mot de passe
curl -X PUT http://localhost:8080/resource/$RESOURCE_ID/env \
  -H "Content-Type: application/json" \
  -d '{
    "POSTGRES_USER": "paastech",
    "POSTGRES_PASSWORD": "nouveau_mdp_securise",
    "POSTGRES_DB": "ma_base"
  }'

# 4. Appliquer en redémarrant
curl -X POST http://localhost:8080/resource/$RESOURCE_ID/stop
curl -X POST http://localhost:8080/resource/$RESOURCE_ID/start
```
