# Configuration

PaaSTech se configure via des variables d'environnement. Le fichier `.env` (copié depuis `.env.example`) est chargé automatiquement au démarrage via la crate `dotenvy`.

## Variables d'environnement

### API

| Variable | Description | Valeur par défaut | Obligatoire |
|---|---|---|---|
| `DATABASE_URL` | URL de connexion PostgreSQL au format `postgresql://user:password@host:port/dbname` | `postgresql://paastech:paastech@localhost:5433/paastech` | Oui |
| `HOST` | Adresse d'écoute de l'API | `127.0.0.1` | Non |
| `PORT` | Port d'écoute de l'API | `8080` | Non |
| `BASE_DOMAIN` | Domaine de base pour le routing Traefik (voir ci-dessous) | `localhost` | Non |

En production (dans un conteneur Docker), `HOST` doit être `0.0.0.0` pour que l'API soit accessible depuis l'extérieur du conteneur. C'est configuré automatiquement dans `compose.prod.yml`.

### PostgreSQL (Docker Compose uniquement)

Ces variables sont utilisées par le service `db` dans `compose.yml` et `compose.prod.yml`. L'API les utilise via `DATABASE_URL`.

| Variable | Description | Valeur par défaut | Obligatoire |
|---|---|---|---|
| `POSTGRES_USER` | Nom d'utilisateur PostgreSQL | `paastech` | Non |
| `POSTGRES_PASSWORD` | Mot de passe PostgreSQL | `paastech` | Non |
| `POSTGRES_DB` | Nom de la base de données | `paastech` | Non |

## Fichier `.env` de référence

```dotenv
# Connexion à la base de données
DATABASE_URL=postgresql://paastech:paastech@localhost:5433/paastech

# Adresse et port d'écoute de l'API
HOST=127.0.0.1
PORT=8080

# Domaine de base pour le routing Traefik
BASE_DOMAIN=localhost

# Configuration PostgreSQL pour Docker Compose
POSTGRES_USER=paastech
POSTGRES_PASSWORD=paastech
POSTGRES_DB=paastech
```

---

## Configuration Traefik

Traefik est le reverse proxy intégré à PaaSTech. Il lit les labels Docker des conteneurs déployés et configure dynamiquement le routing HTTP sans redémarrage.

### Variable `BASE_DOMAIN`

`BASE_DOMAIN` définit le domaine racine sous lequel toutes les applications sont exposées. Chaque application déployée reçoit automatiquement un sous-domaine :

```
http://{nom-application}.{BASE_DOMAIN}/
```

| Environnement | `BASE_DOMAIN` | URL d'accès |
|---|---|---|
| Développement local | `localhost` | `http://mon-app.localhost:8081/` |
| Production | `apps.exemple.fr` | `http://mon-app.apps.exemple.fr/` |

### Ports Traefik

| Environnement | Port HTTP | Dashboard |
|---|---|---|
| Développement (`compose.yml`) | `8081` | `http://localhost:9090` (activé) |
| Production (`compose.prod.yml`) | `80` | Désactivé |

### Configuration CLI Traefik (compose.yml — développement)

```yaml
command:
  - "--providers.docker=true"
  - "--providers.docker.exposedbydefault=false"
  - "--entrypoints.web.address=:80"
  - "--api.insecure=true"
```

| Option | Description |
|---|---|
| `providers.docker=true` | Active la découverte automatique des conteneurs Docker |
| `providers.docker.exposedbydefault=false` | Seuls les conteneurs avec `traefik.enable=true` sont routés |
| `entrypoints.web.address=:80` | Point d'entrée HTTP sur le port 80 du conteneur |
| `api.insecure=true` | Active le dashboard Traefik (développement uniquement) |

### Réseau Docker `paas-net`

Traefik et tous les conteneurs applicatifs sont connectés au réseau Docker `paas-net`. PaaSTech crée ce réseau automatiquement s'il n'existe pas lors du premier déploiement.

```
Traefik ─── paas-net ─── Conteneur app 1
                     ─── Conteneur app 2
                     ─── Conteneur ressource 1
```

---

## Configuration Rust (`.cargo/config.toml`)

```toml
[env]
SQLX_OFFLINE = "true"
```

`SQLX_OFFLINE=true` permet à `sqlx` de vérifier les requêtes SQL à la compilation sans connexion active à la base.

---

## Docker Compose

### Développement (`compose.yml`)

- PostgreSQL exposé sur le port **5433** de l'hôte
- Traefik HTTP sur le port **8081**, dashboard sur **9090**
- Le socket Docker est configurable via `${DOCKER_SOCK:-/var/run/docker.sock}`

### Production (`compose.prod.yml`)

- PostgreSQL non exposé à l'hôte (réseau `internal` uniquement)
- Traefik HTTP sur le port **80** (standard)
- Dashboard Traefik désactivé
- API et Traefik partagent le réseau `paas-net` ; API et PostgreSQL partagent le réseau `internal`
- Le socket Docker `/var/run/docker.sock` est monté dans les conteneurs API et Traefik

---

## Schéma de base de données

Le schéma est défini dans `database/init.sql` et initialisé automatiquement au premier démarrage du conteneur PostgreSQL. Il crée quatre tables :

- `applications` — Registre des applications déployées
- `services` — Ressources managées (PostgreSQL, Redis, S3)
- `application_services` — Association application ↔ ressource
- `service_env_vars` — Variables d'environnement des ressources

Pour réinitialiser manuellement :

```bash
psql $DATABASE_URL -f database/init.sql
```
