# PaasTech

A self-hosted Platform as a Service (PaaS) for deploying and managing containerised applications from the terminal.

## Architecture

- **API** (`src/`) — Actix-web REST API backed by PostgreSQL and Docker (via Bollard)
- **CLI** (`cli/`) — `paastech` command-line client
- **Traefik** — reverse proxy, automatically configured for each deployed application
- **Garage** — S3-compatible object storage; one shared instance, one bucket per S3 resource

## Prerequisites

- Docker
- PostgreSQL
- Rust (for building from source)

## Running the API

```bash
# Copy and edit environment variables
cp .env.example .env

# Start dependencies (PostgreSQL, Traefik)
docker compose up -d

# Run the API
cargo run
```

Environment variables:

| Variable       | Default                                              | Description               |
|----------------|------------------------------------------------------|---------------------------|
| `HOST`         | `127.0.0.1`                                          | Bind address              |
| `PORT`         | `8080`                                               | Bind port                 |
| `DATABASE_URL` | `postgresql://paastech:paastech@localhost:5433/paastech` | PostgreSQL connection URL |
| `GARAGE_IMAGE` | `dxflrs/garage:v2.3.0`                               | Garage Docker image       |

## Installing the CLI

```bash
cd cli
cargo install --path .
```

See [`cli/README.md`](cli/README.md) for configuration and shell completion setup.

## Usage guide

See [`docs/usage.md`](docs/usage.md) for a full walkthrough: deploying apps, managing resources, environment variables, and the complete command reference.

## Development

### Pre-commit hooks

```bash
uv run pre-commit install
```

### API documentation

Swagger UI is available at `http://localhost:8080/swagger-ui/` when the API is running.
