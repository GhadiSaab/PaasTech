# Installation

Deux modes d'installation sont disponibles : **Docker Compose** (recommandé pour la production) et **développement natif** (recommandé pour contribuer au projet).

---

## Option A — Docker Compose (production)

### 1. Cloner le dépôt

```bash
git clone <url-du-depot>
cd paastech
```

### 2. Configurer l'environnement

```bash
cp .env.example .env
```

Éditer `.env` selon les besoins (voir [Configuration](configuration.md)).

### 3. Démarrer la stack complète

```bash
docker compose -f compose.prod.yml up -d
```

Ce compose lance trois services :

| Service | Rôle | Port exposé |
|---|---|---|
| `db` | PostgreSQL 16 (état de la plateforme) | interne uniquement |
| `traefik` | Reverse proxy HTTP (routing des apps) | `80` |
| `api` | L'API PaaSTech | `8080` |

### 4. Vérifier que l'API répond

```bash
curl http://localhost:8080/app
```

Réponse attendue : `[]` (liste vide d'applications).

### 5. Vérifier Traefik

```bash
# Tester le routing Traefik
curl -H "Host: test.localhost" http://localhost/
```

---

## Option B — Développement natif

### 1. Cloner le dépôt

```bash
git clone <url-du-depot>
cd paastech
```

### 2. Configurer l'environnement

```bash
cp .env.example .env
```

### 3. Démarrer PostgreSQL et Traefik

```bash
docker compose up db traefik -d
```

Attendre que PostgreSQL soit prêt :

```bash
docker compose ps
```

La colonne `STATUS` du service `db` doit afficher `healthy`.

### 4. Vérifier la connexion à la base

```bash
psql postgresql://paastech:paastech@localhost:5433/paastech -c "\dt"
```

Les tables `applications`, `services`, `application_services` et `service_env_vars` doivent être présentes (créées automatiquement par `database/init.sql` au premier démarrage).

!!! note "Initialisation manuelle"
    Si la base de données existe mais que les tables sont absentes, les créer manuellement :
    ```bash
    psql postgresql://paastech:paastech@localhost:5433/paastech -f database/init.sql
    ```

### 5. Compiler le projet

```bash
cargo build
```

La variable `SQLX_OFFLINE=true` est définie dans `.cargo/config.toml`, ce qui permet la compilation sans connexion active à la base.

### 6. Démarrer l'API

```bash
cargo run
```

Sortie attendue :

```
Loading PSQL...
Running on http://127.0.0.1:8080
Swagger UI: http://127.0.0.1:8080/swagger-ui/
```

### 7. Vérifier le bon fonctionnement

```bash
# API
curl http://localhost:8080/app

# Traefik dashboard (développement uniquement)
open http://localhost:9090
```

### 8. (Optionnel) Installer les hooks pre-commit

```bash
uv run pre-commit install
```

---

## Option C — Build de production (image Docker)

```bash
docker build -t paastech:latest .
```

Le `Dockerfile` utilise un build multi-étapes :

1. **Stage 1** — `rust:1.88` : compilation du binaire en mode `release`
2. **Stage 2** — `debian:bookworm-slim` : image finale légère avec uniquement le binaire

La compilation nécessite `pkg-config` et `libssl-dev`, installés automatiquement dans le stage 1.

---

## Accès aux applications après déploiement

Une fois la stack démarrée, les applications déployées sont accessibles via Traefik en utilisant le nom de l'application comme sous-domaine :

```
http://{nom-app}.{BASE_DOMAIN}/
```

En développement (`BASE_DOMAIN=localhost`, Traefik sur le port `8081`) :

```bash
# Déployer une app
curl -X POST http://localhost:8080/app/deploy \
  -H "Content-Type: application/json" \
  -d '{"name": "mon-nginx", "image": "nginx:latest", "port": 80}'

# Y accéder via Traefik
curl -H "Host: mon-nginx.localhost" http://localhost:8081/
```

En production (`BASE_DOMAIN=apps.exemple.fr`, Traefik sur le port `80`) :

```bash
curl http://mon-nginx.apps.exemple.fr/
```
