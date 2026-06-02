# Référence API

L'API PaaSTech est une API REST JSON exposée par défaut sur `http://localhost:8080`.

**Interface interactive** : `http://localhost:8080/swagger-ui/`  
**Schéma OpenAPI** : `http://localhost:8080/api-docs/openapi.json`

---

## Services

### Lister les versions disponibles

```
GET /service/{name}/versions
```

Interroge Docker Hub pour récupérer les tags disponibles pour un service.

**Paramètres de chemin**

| Paramètre | Type | Description |
|---|---|---|
| `name` | `string` | Nom du service : `postgres`, `redis`, `s3` |

**Réponses**

| Code | Description |
|---|---|
| `200` | Liste des tags disponibles |
| `400` | Nom de service invalide |
| `500` | Erreur interne |

**Exemple**

```bash
curl http://localhost:8080/service/postgres/versions
```

```json
["16", "15", "14", "13", "16-alpine", "alpine"]
```

---

## Applications

### Lister les applications

```
GET /app
```

**Réponses**

| Code | Description |
|---|---|
| `200` | Tableau d'objets `App` |
| `500` | Erreur interne |

**Exemple**

```bash
curl http://localhost:8080/app
```

```json
[
  {
    "id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
    "name": "mon-nginx",
    "image_id": "nginx:latest",
    "container_id": "d3f1a2b4c5e6",
    "internal_port": 80,
    "port": 32768,
    "status": "running",
    "created_at": "2024-01-15T10:30:00"
  }
]
```

---

### Déployer une application

```
POST /app/deploy
```

Pull une image Docker, configure les labels Traefik et démarre un conteneur sur le réseau `paas-net`. Si un conteneur avec le même nom existe déjà, il est arrêté et remplacé.

**Corps de la requête** (`application/json`)

```json
{
  "name": "mon-app",
  "image": "nginx:latest",
  "port": 80
}
```

| Champ | Type | Description | Obligatoire |
|---|---|---|---|
| `name` | `string` | Nom unique de l'application | Oui |
| `image` | `string` | Image Docker (`nom:tag`) | Oui |
| `port` | `integer` | Port interne du conteneur. Si omis, détecté depuis l'image. | Non |

**Réponses**

| Code | Description |
|---|---|
| `200` | Application déployée |
| `500` | Erreur interne (port ambigu, image introuvable, etc.) |

**Exemple**

```bash
curl -X POST http://localhost:8080/app/deploy \
  -H "Content-Type: application/json" \
  -d '{"name": "mon-app", "image": "nginx:latest"}'
```

Après déploiement, l'application est accessible via Traefik : `http://mon-app.{BASE_DOMAIN}/`

---

### Arrêter une application

```
POST /app/{app_name}/stop
```

**Paramètres de chemin**

| Paramètre | Type | Description |
|---|---|---|
| `app_name` | `string` | Nom de l'application |

**Réponses**

| Code | Description |
|---|---|
| `200` | Application arrêtée |
| `404` | Application introuvable |
| `500` | Erreur interne |

**Exemple**

```bash
curl -X POST http://localhost:8080/app/mon-app/stop
```

---

### Redémarrer une application

```
POST /app/{app_name}/restart
```

Pull l'image, arrête et recrée le conteneur avec de nouveaux labels Traefik horodatés. Voir [Rolling Update](../usage/rolling-update.md).

**Paramètres de chemin**

| Paramètre | Type | Description |
|---|---|---|
| `app_name` | `string` | Nom de l'application |

**Réponses**

| Code | Description |
|---|---|
| `200` | Application redémarrée |
| `404` | Application introuvable |
| `500` | Erreur interne |

**Exemple**

```bash
curl -X POST http://localhost:8080/app/mon-app/restart
```

---

### Obtenir le statut d'une application

```
GET /app/{app_name}/status
```

Interroge Docker en temps réel.

**Paramètres de chemin**

| Paramètre | Type | Description |
|---|---|---|
| `app_name` | `string` | Nom de l'application |

**Réponses**

| Code | Description |
|---|---|
| `200` | Statut en texte brut : `running`, `exited`, `paused`, `unknown` |
| `404` | Application introuvable en base |
| `500` | Erreur interne |

**Exemple**

```bash
curl http://localhost:8080/app/mon-app/status
# => running
```

---

### Upload d'application

```
POST /app/upload
```

Upload d'une archive ZIP contenant le code source d'une application.

**Corps de la requête** (`multipart/form-data`)

| Champ | Type | Description | Obligatoire |
|---|---|---|---|
| `file` | `file` | Archive ZIP de l'application | Oui |
| `name` | `string` | Nom de l'application (défaut : nom du fichier ZIP) | Non |
| `internal_port` | `string` | Port interne du conteneur | Non |

**Réponses**

| Code | Description |
|---|---|
| `200` | Fichier uploadé et traité |
| `400` | Aucun fichier dans le payload |
| `500` | Erreur interne |

**Exemple**

```bash
curl -X POST http://localhost:8080/app/upload \
  -F "file=@mon-app.zip" \
  -F "name=mon-app" \
  -F "internal_port=8000"
```

---

## Ressources

Les ressources sont les services managés : `postgres`, `redis`, `s3`.

### Créer une ressource

```
POST /resource
```

Valide le tag Docker sur Docker Hub, crée le conteneur sur le réseau `paas-net` et le démarre immédiatement.

**Corps de la requête** (`application/json`)

```json
{
  "display_name": "Ma PostgreSQL",
  "name": "postgres",
  "version": "16",
  "application_id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890"
}
```

| Champ | Type | Description | Obligatoire |
|---|---|---|---|
| `display_name` | `string` | Nom affiché de la ressource | Oui |
| `name` | `string` | Type : `postgres`, `redis`, `s3` | Oui |
| `version` | `string` | Tag Docker du service | Oui |
| `application_id` | `string` (UUID) | Application à associer | Non |

**Réponses**

| Code | Description |
|---|---|
| `201` | Ressource créée, objet `Resource` retourné |
| `400` | Nom de service invalide, version invalide, ou `application_id` malformé |
| `500` | Erreur interne |

**Exemple**

```bash
curl -X POST http://localhost:8080/resource \
  -H "Content-Type: application/json" \
  -d '{"display_name": "Ma BDD", "name": "postgres", "version": "16"}'
```

```json
{
  "id": "b2c3d4e5-f6a7-8901-bcde-f12345678901",
  "display_name": "Ma BDD",
  "name": "postgres",
  "version": "16",
  "status": "running",
  "application_ids": []
}
```

---

### Lister les ressources

```
GET /resource
```

**Réponses**

| Code | Description |
|---|---|
| `200` | Tableau d'objets `Resource` |
| `500` | Erreur interne |

**Exemple**

```bash
curl http://localhost:8080/resource
```

---

### Obtenir une ressource

```
GET /resource/{id}
```

**Paramètres de chemin**

| Paramètre | Type | Description |
|---|---|---|
| `id` | `string` (UUID) | Identifiant de la ressource |

**Réponses**

| Code | Description |
|---|---|
| `200` | Objet `Resource` |
| `400` | UUID invalide |
| `404` | Ressource introuvable |
| `500` | Erreur interne |

**Exemple**

```bash
curl http://localhost:8080/resource/b2c3d4e5-f6a7-8901-bcde-f12345678901
```

---

### Mettre à jour une ressource

```
PATCH /resource/{id}
```

Modifie le `display_name`, la `version`, et/ou la liste des applications associées. Les champs omis ne sont pas modifiés (sauf `application_ids` qui remplace toutes les associations existantes).

**Paramètres de chemin**

| Paramètre | Type | Description |
|---|---|---|
| `id` | `string` (UUID) | Identifiant de la ressource |

**Corps de la requête** (`application/json`)

```json
{
  "display_name": "Nouveau nom",
  "version": "15",
  "application_ids": ["a1b2c3d4-e5f6-7890-abcd-ef1234567890"]
}
```

| Champ | Type | Description | Obligatoire |
|---|---|---|---|
| `display_name` | `string` | Nouveau nom affiché | Non |
| `version` | `string` | Nouvelle version (validée sur Docker Hub) | Non |
| `application_ids` | `string[]` | Liste complète des applications associées (remplace) | Non |

**Réponses**

| Code | Description |
|---|---|
| `200` | Ressource mise à jour |
| `400` | UUID invalide ou version invalide |
| `404` | Ressource introuvable |
| `500` | Erreur interne |

**Exemple**

```bash
curl -X PATCH http://localhost:8080/resource/b2c3d4e5-f6a7-8901-bcde-f12345678901 \
  -H "Content-Type: application/json" \
  -d '{"display_name": "PostgreSQL v15"}'
```

---

### Supprimer une ressource

```
DELETE /resource/{id}
```

Supprime la ressource en base (en cascade : associations et variables d'environnement). Arrêter le conteneur avant suppression.

**Paramètres de chemin**

| Paramètre | Type | Description |
|---|---|---|
| `id` | `string` (UUID) | Identifiant de la ressource |

**Réponses**

| Code | Description |
|---|---|
| `204` | Ressource supprimée |
| `400` | UUID invalide |
| `404` | Ressource introuvable |
| `500` | Erreur interne |

**Exemple**

```bash
curl -X DELETE http://localhost:8080/resource/b2c3d4e5-f6a7-8901-bcde-f12345678901
```

---

### Démarrer une ressource

```
POST /resource/{id}/start
```

**Paramètres de chemin**

| Paramètre | Type | Description |
|---|---|---|
| `id` | `string` (UUID) | Identifiant de la ressource |

**Réponses**

| Code | Description |
|---|---|
| `200` | Ressource démarrée |
| `400` | UUID invalide |
| `404` | Ressource introuvable |
| `409` | Ressource déjà en cours d'exécution |
| `500` | Erreur interne |

**Exemple**

```bash
curl -X POST http://localhost:8080/resource/b2c3d4e5-f6a7-8901-bcde-f12345678901/start
```

---

### Arrêter une ressource

```
POST /resource/{id}/stop
```

**Paramètres de chemin**

| Paramètre | Type | Description |
|---|---|---|
| `id` | `string` (UUID) | Identifiant de la ressource |

**Réponses**

| Code | Description |
|---|---|
| `200` | Ressource arrêtée |
| `400` | UUID invalide |
| `404` | Ressource introuvable |
| `409` | Ressource déjà arrêtée |
| `500` | Erreur interne |

**Exemple**

```bash
curl -X POST http://localhost:8080/resource/b2c3d4e5-f6a7-8901-bcde-f12345678901/stop
```

---

### Lire les variables d'environnement

```
GET /resource/{id}/env
```

**Paramètres de chemin**

| Paramètre | Type | Description |
|---|---|---|
| `id` | `string` (UUID) | Identifiant de la ressource |

**Réponses**

| Code | Description |
|---|---|
| `200` | Objet JSON `{ "KEY": "value", ... }` |
| `400` | UUID invalide |
| `404` | Ressource introuvable |
| `500` | Erreur interne |

**Exemple**

```bash
curl http://localhost:8080/resource/b2c3d4e5-f6a7-8901-bcde-f12345678901/env
```

```json
{
  "POSTGRES_DB": "paastech",
  "POSTGRES_PASSWORD": "xK9mP2qR8s",
  "POSTGRES_USER": "paastech"
}
```

---

### Remplacer les variables d'environnement

```
PUT /resource/{id}/env
```

Remplace **toutes** les variables d'environnement existantes. Redémarrer la ressource pour appliquer les changements.

**Paramètres de chemin**

| Paramètre | Type | Description |
|---|---|---|
| `id` | `string` (UUID) | Identifiant de la ressource |

**Corps de la requête** (`application/json`)

```json
{
  "POSTGRES_USER": "mon_user",
  "POSTGRES_PASSWORD": "mon_mdp",
  "POSTGRES_DB": "ma_base"
}
```

**Réponses**

| Code | Description |
|---|---|
| `200` | Variables mises à jour (redémarrage requis pour application) |
| `400` | UUID invalide |
| `404` | Ressource introuvable |
| `500` | Erreur interne |

**Exemple**

```bash
curl -X PUT http://localhost:8080/resource/b2c3d4e5-f6a7-8901-bcde-f12345678901/env \
  -H "Content-Type: application/json" \
  -d '{"POSTGRES_USER": "admin", "POSTGRES_PASSWORD": "secret", "POSTGRES_DB": "prod"}'
```

---

## Modèles de données

### `App`

| Champ | Type | Description |
|---|---|---|
| `id` | `string` (UUID) | Identifiant unique |
| `name` | `string` | Nom de l'application |
| `image_id` | `string \| null` | Image Docker utilisée |
| `container_id` | `string \| null` | ID du conteneur Docker (64 caractères) |
| `internal_port` | `integer \| null` | Port interne du conteneur |
| `port` | `integer \| null` | Port hôte alloué |
| `status` | `string` | `running` ou `stopped` |
| `created_at` | `string` (ISO 8601) | Date de création |

### `Resource`

| Champ | Type | Description |
|---|---|---|
| `id` | `string` (UUID) | Identifiant unique |
| `display_name` | `string` | Nom affiché |
| `name` | `string` | Type de service (`postgres`, `redis`, `s3`) |
| `version` | `string` | Tag Docker |
| `status` | `string` | `running` ou `stopped` |
| `application_ids` | `string[]` | UUIDs des applications associées |
