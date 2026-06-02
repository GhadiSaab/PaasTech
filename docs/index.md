# PaaSTech

**PaaSTech** est une plateforme as a service (PaaS) légère développée en Rust. Elle permet de déployer et d'orchestrer des applications conteneurisées ainsi que des services managés (bases de données PostgreSQL, caches Redis, stockage objet S3) à partir d'une API REST simple.

Le moteur de la plateforme s'appuie sur Docker pour l'exécution des conteneurs, PostgreSQL pour la persistance de l'état, et expose une API [Actix-web](https://actix.rs/) documentée avec Swagger UI.

## Ce que permet PaaSTech

- **Déployer** une application en une seule requête HTTP
- **Gérer le cycle de vie** des conteneurs : démarrage, arrêt, redéploiement
- **Provisionner des ressources managées** : PostgreSQL, Redis, Garage S3 — démarrés automatiquement en tant que conteneurs
- **Configurer les variables d'environnement** des ressources via l'API
- **Consulter l'état** en temps réel de chaque application et ressource
- **Explorer l'API** interactivement via l'interface Swagger UI intégrée

## Liens rapides

<div class="grid cards" markdown>

- :material-rocket-launch: **[Installation](getting-started/installation.md)**  
  Démarrer PaaSTech en 5 minutes

- :material-wrench: **[Configuration](getting-started/configuration.md)**  
  Variables d'environnement et options de configuration

- :material-apps: **[Déployer une application](usage/apps.md)**  
  Créer et gérer des applications

- :material-api: **[Référence API](api/reference.md)**  
  Tous les endpoints REST documentés

</div>

## Vue d'ensemble rapide

```bash
# Déployer une application depuis une image Docker Hub
curl -X POST http://localhost:8080/app/deploy \
  -H "Content-Type: application/json" \
  -d '{"name": "mon-app", "image": "nginx:latest", "port": 80}'

# Lister les applications déployées
curl http://localhost:8080/app

# Créer une base de données PostgreSQL
curl -X POST http://localhost:8080/resource \
  -H "Content-Type: application/json" \
  -d '{"display_name": "Ma BDD", "name": "postgres", "version": "16"}'
```

!!! info "Interface Swagger UI"
    Une interface interactive est disponible à l'adresse `http://localhost:8080/swagger-ui/` une fois l'API démarrée.
