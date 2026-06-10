# PaasTech — Usage Guide

PaasTech is a self-hosted PaaS (Platform as a Service) for deploying and managing containerised applications from the terminal.

## Table of contents

- [Installing the CLI](#installing-the-cli)
- [Configuration](#configuration)
- [Getting started](#getting-started)
- [Deploying an application](#deploying-an-application)
- [Managing resources](#managing-resources)
- [Environment variables](#environment-variables)
- [Command reference](#command-reference)

---

## Installing the CLI

From source:

```bash
cd cli
cargo install --path .
```

---

## Configuration

### API URL

All commands go through the PaasTech API. Point the CLI at your instance:

```bash
export PAAS_API_URL=https://api.example.com
```

Add this line to your `~/.bashrc` or `~/.zshrc` so you don't have to repeat it each session.

Default: `http://127.0.0.1:8080`

---

## Getting started

### 1. Initialise a project

From the root of your repository:

```bash
paastech init my-project
```

This creates a `paastech.toml` file in the current directory and registers the project on the PaaS. All subsequent commands read this file automatically.

### 2. Check the project is recognised

```bash
paastech project info
```

---

## Deploying an application

### Method 1 — From source (recommended)

The PaaS automatically detects the project type (buildpack) and builds it server-side.

```bash
paastech deploy
```

The current directory is archived and sent to the PaaS. The build runs on the server and the result is deployed. The command waits until the application is running before returning.

> `.env`, `.git/`, `target/`, `node_modules/`, and `__pycache__/` are automatically excluded from the archive.

#### Optional manifest (`paastech.toml`)

For multi-process projects or projects with attached resources, enrich your `paastech.toml`:

```toml
[project]
name = "my-project"

[[processes]]
name    = "api"
type    = "web"
path    = "."
port    = 8000

[[processes]]
name    = "worker"
type    = "worker"
path    = "."

[[resources]]
name    = "db"
type    = "postgres"
connection = "asyncpg"

[[resources]]
name    = "cache"
type    = "redis"

[[resources]]
name    = "storage"
type    = "s3"
```

- **`type = "web"`** — HTTP-facing process; receives the `PORT` environment variable
- **`type = "worker"`** — background process, no exposed port
- **`connection`** (postgres only) — `sync` (SQLAlchemy sync) or `asyncpg`

### Method 2 — From an existing Docker image

```bash
paastech app deploy my-api --image nginx:latest --port 80
```

### Method 3 — Upload an archive manually

```bash
paastech app upload --source ./my-project.zip
```

---

## Managing resources

Resources are managed services (database, cache, object storage) attached to your applications.

### Create a resource

```bash
# PostgreSQL
paastech resource create my-db --type postgres --connection sync

# Redis
paastech resource create my-cache --type redis

# S3 object storage (Garage)
paastech resource create my-storage --type s3
```

> A single Garage container is shared across all S3 resources. Each resource gets its own isolated bucket and credentials.

### Create and link to an application in one step

```bash
paastech resource create my-db --type postgres --connection asyncpg --link my-api
```

Connection variables are automatically injected into the linked application.

### Variables injected per resource type

| Type     | Injected variables                                                                                                          |
|----------|-----------------------------------------------------------------------------------------------------------------------------|
| postgres | `POSTGRES_HOST`, `POSTGRES_PORT`, `DATABASE_URL`                                                                            |
| redis    | `REDIS_HOST`, `REDIS_PORT`, `REDIS_URL`                                                                                     |
| s3       | `S3_HOST`, `S3_PORT`, `S3_ENDPOINT_URL`, `S3_REGION`, `S3_ACCESS_KEY_ID`, `S3_SECRET_ACCESS_KEY`, `S3_BUCKET`              |

### Attach an existing resource to an application

```bash
paastech resource attach my-db --app my-api --connection sync
```

### List resources

```bash
paastech resource list
```

### Delete a resource

```bash
paastech resource delete my-db
```

---

## Environment variables

### For an application

```bash
# Set a variable
paastech app env set my-api SECRET_KEY=abc123

# List variables
paastech app env list my-api
```

### For a project (shared by all apps)

```bash
paastech project env set LOG_LEVEL=info
paastech project env list
```

### For a resource

```bash
paastech resource env set my-db MAX_CONNECTIONS=20
```

---

## Command reference

### Projects

| Command                         | Description                          |
|---------------------------------|--------------------------------------|
| `paastech init <name>`          | Initialise a project                 |
| `paastech project info`         | Show the current project             |
| `paastech project list`         | List all projects                    |
| `paastech project delete <name>`| Delete a project                     |
| `paastech project env set K=V`  | Set a project-level variable         |
| `paastech project env list`     | List project-level variables         |

### Applications

| Command                                      | Description                                     |
|----------------------------------------------|-------------------------------------------------|
| `paastech deploy [--port N]`                 | Deploy the current directory                    |
| `paastech app deploy <name> --image <img>`   | Deploy from a Docker image                      |
| `paastech app list`                          | List applications                               |
| `paastech app info <name>`                   | Show application status                         |
| `paastech app stop <name>`                   | Stop an application                             |
| `paastech app restart <name>`                | Restart an application                          |
| `paastech app delete <name>`                 | Delete an application                           |
| `paastech app logs <name> [--tail N] [-f]`   | Show logs (optionally follow)                   |
| `paastech app env set <name> K=V`            | Set an environment variable                     |
| `paastech app env list <name>`               | List environment variables                      |
| `paastech app upload --source <file.zip>`    | Upload an archive manually                      |

### Resources

| Command                                                 | Description                                |
|---------------------------------------------------------|--------------------------------------------|
| `paastech resource create <name> --type <type>`         | Create a resource                          |
| `paastech resource list`                                | List resources                             |
| `paastech resource info <name>`                         | Show resource details                      |
| `paastech resource attach <name> --app <app>`           | Attach a resource to an application        |
| `paastech resource edit <name> [--version V] [--link A]`| Update version or links                   |
| `paastech resource start <name>`                        | Start a stopped resource                   |
| `paastech resource stop <name>`                         | Stop a resource                            |
| `paastech resource delete <name>`                       | Delete a resource and its data             |
| `paastech resource logs <name> [-f]`                    | Show resource logs                         |
| `paastech resource env set <name> K=V`                  | Set a resource environment variable        |
| `paastech resource versions <type>`                     | List available versions from the Hub       |

---

## Shell completion

See the [CLI README](../cli/README.md) for setting up Bash, Zsh, Fish, or PowerShell completion.
