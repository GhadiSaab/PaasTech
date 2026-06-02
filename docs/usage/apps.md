# Applications

Une **application** dans PaaSTech correspond à un conteneur Docker issu d'une image publique. L'API permet de déployer, inspecter, arrêter et redémarrer des applications. Chaque application déployée est automatiquement accessible via Traefik sous `http://{nom-app}.{BASE_DOMAIN}/`.

!!! note "Swagger UI"
    Tous ces endpoints sont explorables interactivement à l'adresse `http://localhost:8080/swagger-ui/`.

---

## Déployer une application

```
POST /app/deploy
```

Tire une image Docker Hub, crée un conteneur sur le réseau `paas-net` et configure automatiquement le routing Traefik.

### Corps de la requête

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
| `port` | `integer` | Port interne du conteneur. Si omis, PaaSTech détecte le port exposé par l'image. | Non |

### Exemple

```bash
curl -X POST http://localhost:8080/app/deploy \
  -H "Content-Type: application/json" \
  -d '{"name": "mon-nginx", "image": "nginx:latest", "port": 80}'
```

Après déploiement, l'application est accessible via :
- Traefik : `http://mon-nginx.localhost:8081/` (développement)
- Traefik : `http://mon-nginx.apps.exemple.fr/` (production avec `BASE_DOMAIN=apps.exemple.fr`)
- Port direct : `http://localhost:{port_hôte}/`

Le champ `name` est soumis à une contrainte d'unicité en base. Un redéploiement sur un nom existant arrête et remplace automatiquement le conteneur précédent.

---

## Lister les applications

```
GET /app
```

### Exemple

```bash
curl http://localhost:8080/app
```

### Réponse

```json
[
  {
    "id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
    "name": "mon-nginx",
    "image_id": "nginx:latest",
    "container_id": "d3f1a2b4c5e6...",
    "internal_port": 80,
    "port": 32768,
    "status": "running",
    "created_at": "2024-01-15T10:30:00"
  }
]
```

| Champ | Description |
|---|---|
| `id` | UUID de l'application |
| `name` | Nom de l'application |
| `image_id` | Image Docker utilisée |
| `container_id` | ID du conteneur Docker |
| `internal_port` | Port interne du conteneur |
| `port` | Port hôte alloué dynamiquement |
| `status` | `running` ou `stopped` |
| `created_at` | Date de création |

---

## Obtenir le statut d'une application

```
GET /app/{app_name}/status
```

Interroge Docker en temps réel (contrairement à `GET /app` qui lit la base).

### Exemple

```bash
curl http://localhost:8080/app/mon-nginx/status
```

### Réponse

Texte brut : `running`, `exited`, `paused`, ou `unknown` si le conteneur est introuvable dans Docker.

---

## Arrêter une application

```
POST /app/{app_name}/stop
```

Arrête le conteneur. Traefik retire automatiquement la règle de routing associée dès que le label `traefik.enable=true` disparaît.

### Exemple

```bash
curl -X POST http://localhost:8080/app/mon-nginx/stop
```

---

## Redémarrer une application

```
POST /app/{app_name}/restart
```

Arrête, supprime et recrée le conteneur depuis la même image avec de nouveaux labels Traefik. Voir [Rolling Update](rolling-update.md) pour les détails de la mécanique.

### Exemple

```bash
curl -X POST http://localhost:8080/app/mon-nginx/restart
```

---

## Exemple end-to-end

```bash
# 1. Déployer une application
curl -X POST http://localhost:8080/app/deploy \
  -H "Content-Type: application/json" \
  -d '{"name": "demo", "image": "httpd:2.4", "port": 80}'

# 2. Vérifier qu'elle tourne
curl http://localhost:8080/app/demo/status
# => running

# 3. Accéder à l'application via Traefik (développement)
curl -H "Host: demo.localhost" http://localhost:8081/
# => <html><body><h1>It works!</h1></body></html>

# 4. Consulter le port hôte alloué (accès direct, sans Traefik)
PORT=$(curl -s http://localhost:8080/app | python3 -c "
import sys, json
apps = json.load(sys.stdin)
print(next(a['port'] for a in apps if a['name'] == 'demo'))
")
curl http://localhost:$PORT/

# 5. Arrêter l'application
curl -X POST http://localhost:8080/app/demo/stop

# 6. Vérifier le statut
curl http://localhost:8080/app/demo/status
# => exited
```
