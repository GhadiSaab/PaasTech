# PaaSTech

Plateforme as a Service légère écrite en Rust. Déploie et orchestre des applications conteneurisées et des services managés (PostgreSQL, Redis, S3) via une API REST, avec routing HTTP automatique assuré par Traefik.

```
curl -X POST http://localhost:8080/app/deploy \
  -H "Content-Type: application/json" \
  -d '{"name": "mon-app", "image": "nginx:latest"}'
# → accessible sur http://mon-app.localhost:8081/
```

---

## Démarrage rapide

**Prérequis** : Docker Engine 20.10+, Docker Compose v2+

```bash
git clone <url-du-depot>
cd paastech
cp .env.example .env
docker compose up db traefik -d
cargo run
```

L'API écoute sur `http://localhost:8080` · Traefik sur `http://localhost:8081` · Dashboard Traefik sur `http://localhost:9090`

> **En production** : `docker compose -f compose.prod.yml up -d`

---

## Test typique — scénario end-to-end

Ce scénario déploie une application web, provisionne une base de données, vérifie le routing Traefik et effectue un rolling update.

### 1. Vérifier que l'API est prête

```bash
curl http://localhost:8080/app
# => []
```

### 2. Déployer une première application

```bash
curl -X POST http://localhost:8080/app/deploy \
  -H "Content-Type: application/json" \
  -d '{"name": "demo", "image": "nginx:latest"}'
```

PaaSTech pull l'image, alloue un port, connecte le conteneur au réseau `paas-net` et configure Traefik automatiquement.

### 3. Accéder à l'application via Traefik

```bash
curl -H "Host: demo.localhost" http://localhost:8081/
# => <!DOCTYPE html> ... <h1>Welcome to nginx!</h1>
```

### 4. Vérifier l'état via l'API

```bash
curl http://localhost:8080/app
```

```json
[{
  "name": "demo",
  "image_id": "nginx:latest",
  "port": 32768,
  "status": "running"
}]
```

```bash
curl http://localhost:8080/app/demo/status
# => running
```

### 5. Provisionner une base de données PostgreSQL

```bash
curl -X POST http://localhost:8080/resource \
  -H "Content-Type: application/json" \
  -d '{"display_name": "Demo DB", "name": "postgres", "version": "16"}'
```

```json
{
  "id": "b2c3d4e5-...",
  "name": "postgres",
  "version": "16",
  "status": "running"
}
```

### 6. Récupérer les credentials générés

```bash
curl http://localhost:8080/resource/b2c3d4e5-.../env
```

```json
{
  "POSTGRES_DB": "paastech",
  "POSTGRES_PASSWORD": "xK9mP2qR8s",
  "POSTGRES_USER": "paastech"
}
```

### 7. Se connecter à la base (via le port hôte alloué)

```bash
# Récupérer le port
PORT=$(curl -s http://localhost:8080/resource/b2c3d4e5-... \
  | python3 -c "import sys,json; print(json.load(sys.stdin)['port'])")

psql postgresql://paastech:xK9mP2qR8s@localhost:$PORT/paastech -c "\dt"
```

### 8. Rolling update — redéployer l'application

```bash
curl -X POST http://localhost:8080/app/demo/restart
```

PaaSTech pull la dernière version de l'image, recrée le conteneur avec de nouveaux labels Traefik horodatés et rétablit la route automatiquement.

```bash
# L'application est de nouveau accessible sans changer l'URL
curl -H "Host: demo.localhost" http://localhost:8081/
# => <h1>Welcome to nginx!</h1>
```

### 9. Arrêter et nettoyer

```bash
# Arrêter l'application
curl -X POST http://localhost:8080/app/demo/stop

# Arrêter et supprimer la base de données
curl -X POST http://localhost:8080/resource/b2c3d4e5-.../stop
curl -X DELETE http://localhost:8080/resource/b2c3d4e5-...
```

---

## Explorer l'API

L'interface Swagger UI est disponible à l'adresse `http://localhost:8080/swagger-ui/` — toutes les routes sont documentées et testables directement depuis le navigateur.

---

## Documentation complète

La documentation exhaustive est disponible dans le dossier [`docs/`](docs/) :

| Page | Contenu |
|---|---|
| [Prérequis](docs/getting-started/prerequisites.md) | Versions requises, OS supportés |
| [Installation](docs/getting-started/installation.md) | Démarrage natif et Docker Compose |
| [Configuration](docs/getting-started/configuration.md) | Variables d'environnement, Traefik, schéma SQL |
| [Applications](docs/usage/apps.md) | Déployer et gérer des applications |
| [Instances](docs/usage/instances.md) | Cycle de vie des conteneurs |
| [Variables d'environnement](docs/usage/env-vars.md) | Configurer les ressources |
| [Bases de données](docs/usage/databases.md) | Provisionner PostgreSQL, Redis, S3 |
| [Rolling Update](docs/usage/rolling-update.md) | Mécanique de redéploiement et labels Traefik |
| [Référence API](docs/api/reference.md) | Tous les endpoints documentés |
| [Architecture](docs/architecture/overview.md) | Schémas et flux de déploiement |
| [Composants](docs/architecture/components.md) | Détail des modules Rust |
| [Contribuer](docs/contributing/development.md) | Setup dev, tests, conventions |

---

## Développement

```bash
# Lancer les tests (PostgreSQL requis)
docker compose up db -d
cargo nextest run

# Linter et formatage
cargo fmt --check
cargo clippy -- -D warnings

# Audit des dépendances
cargo audit
```

```bash
# Installer les hooks pre-commit
uv run pre-commit install
```
