# Contribuer au projet

## Pré-requis pour le développement

- Rust 1.88+ (édition 2024)
- Docker Engine 20.10+
- Docker Compose v2+
- PostgreSQL 16 (ou via `docker compose up db`)
- `cargo-nextest` : `cargo install cargo-nextest`
- `uv` (optionnel, pour les hooks pre-commit)

---

## Démarrage en mode développement

### 1. Cloner et configurer

```bash
git clone <url-du-depot>
cd paastech
cp .env.example .env
```

### 2. Lancer PostgreSQL

```bash
docker compose up db -d
```

### 3. Démarrer l'API avec rechargement automatique

```bash
cargo run
```

Pour un rechargement automatique lors des modifications de code, utiliser [cargo-watch](https://github.com/watchexec/cargo-watch) :

```bash
cargo install cargo-watch
cargo watch -x run
```

### 4. Installer les hooks pre-commit (optionnel)

```bash
uv run pre-commit install
```

Les hooks vérifient le formatage et le linting à chaque commit.

---

## Lancer les tests

Les tests sont des tests d'intégration qui nécessitent une base PostgreSQL active et un daemon Docker.

### Prérequis

```bash
# PostgreSQL doit être en cours d'exécution
docker compose up db -d

# Le .env doit pointer vers la base de test
cat .env
# DATABASE_URL=postgresql://paastech:paastech@localhost:5433/paastech
```

### Lancer tous les tests

```bash
cargo nextest run
```

### Lancer avec le profil CI

```bash
cargo nextest run --profile ci
```

Le profil CI génère un rapport JUnit XML dans `target/nextest/ci/junit.xml`.

### Lancer un test spécifique

```bash
cargo nextest run test_list_apps
```

### Tests disponibles

Les tests couvrent (`src/tests.rs`) :

- Connectivité base de données
- CRUD applications (list, deploy, stop, restart, status)
- CRUD ressources (list, get, create, update, delete)
- Variables d'environnement des ressources (GET, PUT)
- Gestion des conflits d'état (409 Conflict pour start/stop)
- Validation des UUIDs (400 Bad Request)

---

## Qualité du code

### Formatage

```bash
cargo fmt
```

Vérifier sans modifier :

```bash
cargo fmt --check
```

### Linting (Clippy)

```bash
cargo clippy -- -D warnings
```

Le flag `-D warnings` traite tous les warnings comme des erreurs (comme en CI).

### Audit de sécurité

```bash
cargo audit
```

La configuration dans `.cargo/audit.toml` ignore les CVEs non applicables documentées.

---

## Pipeline CI/CD

Le pipeline GitLab CI (`.gitlab-ci.yml`) comprend 3 stages :

| Stage | Jobs |
|---|---|
| `validate` | `fmt` — vérification du formatage Rust |
| `validate` | `clippy` — analyse statique |
| `test` | `test` — tests d'intégration (avec PostgreSQL + Docker-in-Docker) |
| `test` | `audit` — audit des dépendances |
| `publish` | `pages` — génération de la documentation |
| `publish` | `docker-build` — build et push de l'image Docker (branche main) |
| `publish` | `docker-tag` — tag de l'image (sur les tags Git) |

### Lancer le pipeline localement

```bash
# Vérifier lint
cargo fmt --check && cargo clippy -- -D warnings

# Lancer les tests
cargo nextest run --profile ci

# Vérifier la compilation release
cargo build --release
```

---

## Structure des branches Git

Les merges vers `main` déclenchent automatiquement :
- Le build et push de l'image Docker vers le registre CI
- La mise à jour de la documentation publiée sur GitLab Pages

---

## Ajouter un nouveau service managé

Pour ajouter un service de type `mongodb` par exemple :

### 1. Ajouter l'entrée dans `services/services.json`

```json
{
  "mongodb": {
    "docker_image": "library/mongo",
    "container_image": "mongo",
    "port": 27017,
    "env_vars": [
      { "Static": { "key": "MONGO_INITDB_ROOT_USERNAME", "value": "paastech" } },
      { "Generated": { "key": "MONGO_INITDB_ROOT_PASSWORD", "generate": "random" } }
    ],
    "config_file": null
  }
}
```

### 2. Vérifier que le service est reconnu

```bash
cargo run &
curl http://localhost:8080/service/mongodb/versions
```

Aucune modification du code Rust n'est nécessaire — le registre de services est chargé dynamiquement depuis `services.json`.

---

## Conventions de code

- **Édition Rust** : 2024
- **Formatage** : `rustfmt` (configuration par défaut)
- **Linting** : clippy avec `-D warnings` (zéro warning en CI)
- **Requêtes SQL** : `sqlx::query!` et `sqlx::query_as!` pour la vérification à la compilation
- **Gestion des erreurs** : `Result<T, actix_web::Error>` pour les handlers, propagation avec `?` et `map_err()`
- **Tests** : `actix_web::test` harness + connexion PostgreSQL réelle (pas de mocks)

---

## Variables d'environnement pour les tests

Les tests lisent le `DATABASE_URL` depuis `.env`. Pour isoler les tests de la base de développement :

```bash
# Utiliser une base dédiée aux tests
DATABASE_URL=postgresql://paastech:paastech@localhost:5433/paastech_test cargo nextest run
```

---

## Générer la documentation Rust

```bash
cargo doc --no-deps --open
```

La documentation API interne (types, fonctions) est accessible à l'adresse affichée par le navigateur.
