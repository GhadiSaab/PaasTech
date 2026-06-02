# Composants

Description détaillée des modules constitutifs de PaaSTech.

---

## API — `src/main.rs`

**Rôle** : Couche HTTP. Expose 16 endpoints REST, orchestre les appels aux autres modules, et sert l'interface Swagger UI.

**Technologies** : [Actix-web](https://actix.rs/) 4, [utoipa](https://github.com/juhaku/utoipa) pour la génération OpenAPI, [utoipa-swagger-ui](https://github.com/juhaku/utoipa) pour l'interface interactive.

**Structure** : Tous les handlers sont définis dans `main.rs`. Chaque handler est annoté avec `#[utoipa::path(...)]` pour la génération automatique du schéma OpenAPI.

**Initialisation** :
```
main()
  └── init()                    ← charge .env, crée /tmp/uploads
  └── PgPool::connect()         ← connexion PostgreSQL
  └── Scheduler::new()          ← connexion Docker
  └── HttpServer::new()         ← démarrage du serveur HTTP
```

**État partagé** (via `web::Data<T>`) :
- `PgPool` — pool de connexions PostgreSQL
- `Scheduler` — instance Docker
- `Client` (reqwest) — client HTTP pour Docker Hub

---

## Traefik

**Rôle** : Reverse proxy HTTP. Route le trafic entrant vers les conteneurs applicatifs selon le sous-domaine (`Host` header).

**Version** : Traefik v3.6.1

**Intégration Docker** : Traefik écoute les événements Docker via `/var/run/docker.sock`. Quand un conteneur avec le label `traefik.enable=true` démarre, Traefik crée automatiquement une règle de routing. Quand il s'arrête, la règle est retirée.

**Réseau** : Traefik et tous les conteneurs gérés par PaaSTech sont connectés au réseau Docker `paas-net`. Ce réseau est créé automatiquement par PaaSTech si nécessaire.

**Labels Docker générés par PaaSTech** (via `traefik_labels()` dans `scheduler.rs`) :

```
traefik.enable=true
traefik.http.routers.{app_name}.rule=Host(`{app_name}.{BASE_DOMAIN}`)
traefik.http.routers.{app_name}.service={app_name}-{unix_timestamp}
traefik.http.services.{app_name}-{unix_timestamp}.loadbalancer.server.port={internal_port}
```

Le service name est horodaté pour éviter les conflits de configuration Traefik entre les redéploiements successifs.

**Ports exposés** :

| Environnement | HTTP | Dashboard |
|---|---|---|
| Développement | `8081` | `9090` |
| Production | `80` | Désactivé |

**Configuration CLI** (développement) :
```
--providers.docker=true
--providers.docker.exposedbydefault=false
--entrypoints.web.address=:80
--api.insecure=true
```

---

## Scheduler — `src/scheduler.rs`

**Rôle** : Abstraction sur Docker Engine. Responsable de tout ce qui touche aux conteneurs.

**Technologies** : [bollard](https://github.com/fussybeaver/bollard) (client Docker Rust natif), [tokio](https://tokio.rs/) pour l'async.

**Connexion Docker** : Via le socket Unix `/var/run/docker.sock` (par défaut bollard).

**Méthodes principales** :

| Méthode | Description |
|---|---|
| `deploy(pool, name, image, port)` | Pull + création + démarrage d'un conteneur applicatif avec labels Traefik |
| `redeploy(pool, name, image, port, host_port)` | Pull → stop → remove → create avec nouveaux labels → start |
| `stop(name)` | Stop + remove d'un conteneur applicatif |
| `inspect(name)` | Retourne l'état Docker en temps réel |
| `list()` | Liste tous les conteneurs |
| `start_service(id, image, port, env, binds)` | Crée et démarre un conteneur de ressource sur `paas-net` |
| `stop_service(id)` | Stop + remove d'un conteneur de ressource |
| `pull(image)` | Pull d'une image Docker, ajoute `:latest` si tag absent |
| `resolve_internal_port(image, port)` | Détecte le port TCP exposé par l'image si non fourni |
| `ensure_paas_net()` | Crée le réseau `paas-net` s'il n'existe pas |
| `watch(pool)` | Polling 5s → détecte les crashes → redémarre automatiquement |

**Allocation de ports** : `find_free_port()` lie temporairement un socket TCP sur le port 0 pour obtenir un port libre du kernel.

**Détection automatique du port** : `resolve_internal_port()` inspecte l'image Docker (`inspect_image`) et extrait les ports TCP exposés. Fonctionne uniquement si l'image expose exactement un port TCP.

---

## Registry — `src/registry.rs`

**Rôle** : Couche d'accès aux données pour les **applications** uniquement.

**Technologies** : [sqlx](https://github.com/launchbadge/sqlx) avec macros `query_as!` pour la vérification des types à la compilation.

**Structure `App`** :

```rust
pub struct App {
    pub id: Uuid,
    pub name: String,
    pub image_id: Option<String>,
    pub container_id: Option<String>,
    pub internal_port: Option<i32>,
    pub port: Option<i32>,
    pub status: Option<String>,
    pub created_at: Option<NaiveDateTime>,
}
```

**Méthodes disponibles** :

| Méthode | SQL | Description |
|---|---|---|
| `save(pool, ...)` | `INSERT INTO applications` | Créer une application |
| `get(pool, name)` | `SELECT WHERE name = $1` | Lire une application |
| `list(pool)` | `SELECT ORDER BY created_at ASC` | Lister toutes les applications |
| `delete(pool, name)` | `DELETE WHERE name = $1` | Supprimer (non exposé via HTTP) |
| `update_status(pool, name, status)` | `UPDATE SET status` | Mettre à jour le statut |
| `update_container_id(pool, name, id)` | `UPDATE SET container_id` | Mettre à jour l'ID conteneur |

---

## Service Config Registry — `src/docker.rs`

**Rôle** : Registre statique des services supportés (postgres, redis, s3). Fournit la configuration Docker pour chaque type de service.

**Données** : Chargées au démarrage depuis `services/services.json` via `include_str!()` (embarqué dans le binaire).

**Services déclarés** :

| Service | Image Docker | Port conteneur |
|---|---|---|
| `postgres` | `library/postgres` | `5432` |
| `redis` | `library/redis` | `6379` |
| `s3` | `dxflrs/garage` | `3900` |

**Fonctions exposées** :

| Fonction | Description |
|---|---|
| `is_valid_service(name)` | Vérifie si le service est dans le registre |
| `valid_services()` | Liste triée des services disponibles |
| `docker_image_for_service(name)` | Image Docker Hub (ex: `library/postgres`) |
| `container_image_for_service(name)` | Nom de l'image conteneur (ex: `postgres`) |
| `service_port_for_service(name)` | Port par défaut du service |
| `default_env_vars_for_service(name)` | Variables d'environnement avec génération de secrets |
| `prepare_config_for_service(name, id)` | Génère les fichiers de config et retourne les bind mounts |
| `validate_docker_tag(client, image, tag)` | Vérifie l'existence d'un tag sur Docker Hub |
| `fetch_service_versions(client, name)` | Récupère les tags disponibles sur Docker Hub |

**Génération de secrets** : Les variables marquées `"generate": "random"` dans `services.json` sont générées aléatoirement à la création de la ressource.

---

## Engine — `src/engine.rs`

**Rôle** : Traitement des uploads de code source.

**Fonctions** :

| Fonction | Description |
|---|---|
| `save_multipart_file(payload)` | Parse le body multipart, sauve le fichier dans `/tmp/uploads/` |
| `extract_zip(source)` | Extrait l'archive ZIP dans un dossier `{nom}-extract` |
| `launch_code(from)` | Lance `python3 app.py` comme sous-processus |

---

## PostgreSQL

**Rôle** : Persistance de l'état de la plateforme.

**Ce qui est stocké** :

```sql
CREATE TABLE applications (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name VARCHAR(50) NOT NULL UNIQUE,
    image_id varchar(64) NULL,
    container_id varchar(64) NULL,
    internal_port INTEGER NULL,
    port INTEGER NULL,
    status VARCHAR(20) NOT NULL DEFAULT 'stopped',
    created_at TIMESTAMP NOT NULL DEFAULT now()
);

CREATE TABLE services (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    display_name VARCHAR(50) NOT NULL,
    name VARCHAR(50) NOT NULL,
    version VARCHAR(12) NOT NULL,
    container_id varchar(64) NULL,
    port INTEGER NULL,
    status VARCHAR(20) NOT NULL DEFAULT 'stopped',
    created_at TIMESTAMP NOT NULL DEFAULT now()
);

CREATE TABLE application_services (
    application_id UUID NOT NULL,
    service_id UUID NOT NULL,
    PRIMARY KEY (application_id, service_id),
    FOREIGN KEY (application_id) REFERENCES applications(id) ON DELETE CASCADE,
    FOREIGN KEY (service_id) REFERENCES services(id) ON DELETE CASCADE
);

CREATE TABLE service_env_vars (
    service_id UUID NOT NULL REFERENCES services(id) ON DELETE CASCADE,
    key VARCHAR(255) NOT NULL,
    value TEXT NOT NULL,
    PRIMARY KEY (service_id, key)
);
```

**Connexion** : Pool de connexions sqlx (`PgPool`), partagé entre tous les handlers via `web::Data<PgPool>`.

---

## Docker Hub

**Rôle** : Registre d'images publiques. Interrogé pour :
- Valider les tags avant création (`/v2/repositories/{image}/tags/{tag}/`)
- Lister les versions disponibles (`/v2/repositories/{image}/tags/`)

**Client HTTP** : [reqwest](https://github.com/seanmonstar/reqwest) avec TLS rustls. Partagé entre tous les handlers via `web::Data<Client>`.
