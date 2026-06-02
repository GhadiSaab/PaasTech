# Vue d'ensemble de l'architecture

PaaSTech est structurée autour de trois couches : une **API REST** pour exposer les opérations, un **moteur d'orchestration** pour interagir avec Docker, et une **couche de persistance** PostgreSQL pour l'état de la plateforme. Traefik assure le routing HTTP vers les applications déployées.

---

## Flux d'une requête de déploiement

```mermaid
sequenceDiagram
    actor User as Utilisateur
    participant API as API Actix-web<br/>:8080
    participant PG as PostgreSQL<br/>:5433
    participant Docker as Docker Engine
    participant Traefik as Traefik<br/>:80
    participant App as Conteneur app

    User->>API: POST /app/deploy<br/>{"name": "app", "image": "nginx:latest"}
    API->>Docker: pull("nginx:latest")
    Docker-->>API: Image prête
    API->>Docker: create_container<br/>network: paas-net<br/>labels: traefik.enable=true<br/>Host(`app.localhost`)
    API->>Docker: start_container
    Docker-->>API: container_id
    Note over Traefik: Détecte les labels Docker<br/>Active la règle de routing
    API->>PG: INSERT INTO applications
    API-->>User: 200 OK

    User->>Traefik: GET http://app.localhost/
    Traefik->>App: HTTP forward (paas-net)
    App-->>Traefik: Réponse
    Traefik-->>User: Réponse
```

---

## Schéma des composants

```mermaid
graph TB
    subgraph Client
        U[Utilisateur / CLI / curl]
    end

    subgraph PaaSTech
        API["API Actix-web<br/>(main.rs) :8080"]
        SCH["Scheduler<br/>(scheduler.rs)"]
        REG["Registry<br/>(registry.rs)"]
        DOCKER_CFG["Service Config Registry<br/>(docker.rs + services.json)"]
    end

    subgraph Storage
        PG[(PostgreSQL)]
    end

    subgraph Runtime
        DOCKER[Docker Engine]
        NET["Réseau paas-net"]
        APP_C[Conteneurs applicatifs]
        SVC_C[Conteneurs de ressources]
        TRK["Traefik v3.6.1 :80"]
    end

    subgraph External
        HUB[Docker Hub]
    end

    U -->|HTTP REST| API
    U -->|HTTP apps| TRK
    API --> SCH
    API --> REG
    API --> DOCKER_CFG
    REG -->|sqlx| PG
    SCH -->|bollard| DOCKER
    DOCKER -->|labels Docker| TRK
    TRK ---|paas-net| APP_C
    TRK ---|paas-net| SVC_C
    DOCKER -->|gère| APP_C
    DOCKER -->|gère| SVC_C
    APP_C --- NET
    SVC_C --- NET
    API -->|reqwest| HUB
```

---

## Rôle de chaque composant

### API (`main.rs`)

Point d'entrée unique. Reçoit les requêtes HTTP, orchestre les appels aux autres modules, retourne les réponses JSON. Expose une interface Swagger UI.

### Scheduler (`scheduler.rs`)

Couche d'abstraction sur Docker via [bollard](https://github.com/fussybeaver/bollard). Responsable de : pull d'images, génération des labels Traefik, création/démarrage/arrêt des conteneurs sur le réseau `paas-net`, inspection d'état, surveillance et auto-restart.

### Traefik

Reverse proxy HTTP en écoute sur le port 80 (production) ou 8081 (développement). Il surveille les labels Docker des conteneurs sur le réseau `paas-net` et configure dynamiquement les règles de routing. Chaque application est exposée sous `http://{nom-app}.{BASE_DOMAIN}/`.

### Registry (`registry.rs`)

Couche d'accès aux données pour les **applications**. Encapsule les requêtes SQL sur la table `applications` via [sqlx](https://github.com/launchbadge/sqlx).

### Service Config Registry (`docker.rs`)

Registre statique des services supportés, chargé depuis `services/services.json`. Fournit les images Docker, ports par défaut, variables d'environnement et fichiers de configuration pour chaque type de service.

---

## Flux de déploiement détaillé

```mermaid
flowchart TD
    A[POST /app/deploy] --> B[Scheduler.deploy]
    B --> C[Docker pull image]
    C --> D{Port fourni ?}
    D -->|Non| E[Inspecter l'image<br/>détecter le port TCP exposé]
    D -->|Oui| F[Utiliser le port fourni]
    E --> F
    F --> G[Arrêter et supprimer<br/>l'ancien conteneur si existant]
    G --> H[find_free_port]
    H --> I[Générer labels Traefik<br/>service name horodaté]
    I --> J[create_container<br/>paas-net + labels + port binding]
    J --> K[start_container]
    K --> L[Registry.save<br/>INSERT applications]
    L --> M[200 OK]
    M --> N{Traefik}
    N -->|Détecte labels| O[Active route<br/>http://app.BASE_DOMAIN/]
```

---

## Persistance de l'état

Toutes les décisions d'orchestration sont persistées dans PostgreSQL :

| Table | Contenu |
|---|---|
| `applications` | Nom, image, container_id, port, statut |
| `services` | Nom, version, container_id, port, statut |
| `application_services` | Associations application ↔ ressource |
| `service_env_vars` | Variables d'environnement des ressources |

En cas de redémarrage de l'API, l'état est restauré depuis la base. Les conteneurs Docker qui tournaient avant le redémarrage continuent de tourner et Traefik maintient ses routes tant que les conteneurs sont actifs.

---

## Accès réseau aux applications

```
Utilisateur ──HTTP──→ Traefik :80 ──[paas-net]──→ Conteneur :{port_interne}
                      (Host: app.BASE_DOMAIN)
```

L'accès direct via port hôte reste possible pour les protocoles non-HTTP :

```
Utilisateur ──TCP──→ hôte:{port_hôte} ──→ Conteneur:{port_interne}
```
