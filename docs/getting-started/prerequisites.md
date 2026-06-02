# Prérequis

## Dépendances système

### Docker Engine

PaaSTech orchestre les conteneurs en communiquant directement avec le daemon Docker via le socket Unix `/var/run/docker.sock`. Docker doit être installé et actif sur la machine hôte.

| Composant | Version minimale |
|---|---|
| Docker Engine | 20.10+ |
| Docker Compose | v2.0+ (`docker compose`, sans tiret) |

Le processus PaaSTech doit avoir accès en lecture/écriture à `/var/run/docker.sock`. En production, il tourne dans un conteneur avec le socket monté en volume (voir `compose.prod.yml`).

### Rust (développement natif)

Pour compiler et exécuter PaaSTech directement sur la machine (hors Docker) :

| Composant | Version minimale |
|---|---|
| Rust toolchain | 1.88+ |
| Cargo | inclus avec Rust |
| Edition | 2024 |

Installer via [rustup](https://rustup.rs/) :

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

### PostgreSQL

PaaSTech stocke son état dans une base PostgreSQL.

| Composant | Version minimale |
|---|---|
| PostgreSQL | 16 |

En développement, PostgreSQL est lancé via Docker Compose (voir [Installation](installation.md)). Pour un déploiement natif, un serveur PostgreSQL accessible est requis.

### Outils optionnels

| Outil | Usage | Installation |
|---|---|---|
| `cargo-nextest` | Runner de tests (CI) | `cargo install cargo-nextest` |
| `uv` | Gestionnaire Python pour les hooks pre-commit | [uv.astral.sh](https://github.com/astral-sh/uv) |
| `cargo-audit` | Audit des dépendances Rust | `cargo install cargo-audit` |

## Systèmes d'exploitation supportés

| OS | Support |
|---|---|
| Linux (x86_64) | Supporté et testé |
| macOS | Supporté (Docker Desktop requis) |
| Windows | Non testé |

Linux est l'environnement cible de la production. Le socket Docker `/var/run/docker.sock` est un socket Unix, disponible nativement sur Linux. Sur macOS, Docker Desktop expose un socket compatible.

## Accès réseau

L'API écoute par défaut sur `127.0.0.1:8080`. Traefik écoute sur le port `80` (production) ou `8081` (développement) et route le trafic HTTP vers les conteneurs applicatifs via le réseau Docker `paas-net`.
