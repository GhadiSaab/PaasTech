BEGIN;

CREATE TABLE IF NOT EXISTS projects (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name VARCHAR(50) NOT NULL UNIQUE,
    network_name VARCHAR(128) NOT NULL UNIQUE,
    created_at TIMESTAMP NOT NULL DEFAULT now()
);

INSERT INTO projects (id, name, network_name)
VALUES ('00000000-0000-0000-0000-000000000001', 'default', 'paastech-default')
ON CONFLICT (name) DO NOTHING;

CREATE TABLE IF NOT EXISTS applications (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id UUID NOT NULL DEFAULT '00000000-0000-0000-0000-000000000001' REFERENCES projects(id) ON DELETE CASCADE,
    name VARCHAR(50) NOT NULL,
    image_id varchar(64) NULL,
    container_id varchar(64) NULL,
    internal_port INTEGER NULL,
    port INTEGER NULL,
    status VARCHAR(20) NOT NULL DEFAULT 'stopped',
    base_domain VARCHAR(255) NULL,
    created_at TIMESTAMP NOT NULL DEFAULT now(),
    UNIQUE(project_id, name)
);

CREATE TABLE IF NOT EXISTS services (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id UUID NOT NULL DEFAULT '00000000-0000-0000-0000-000000000001' REFERENCES projects(id) ON DELETE CASCADE,
    display_name VARCHAR(50) NOT NULL,
    name VARCHAR(50) NOT NULL,
    version VARCHAR(12) NOT NULL,
    container_id varchar(64) NULL,
    port INTEGER NULL,
    status VARCHAR(20) NOT NULL DEFAULT 'stopped',
    created_at TIMESTAMP NOT NULL DEFAULT now(),
    UNIQUE(project_id, display_name)
);

CREATE TABLE IF NOT EXISTS application_services (
    application_id UUID NOT NULL,
    service_id UUID NOT NULL,
    PRIMARY KEY (application_id, service_id),
    FOREIGN KEY (application_id) REFERENCES applications(id) ON DELETE CASCADE,
    FOREIGN KEY (service_id) REFERENCES services(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS application_processes (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    application_id UUID NOT NULL REFERENCES applications(id) ON DELETE CASCADE,
    name VARCHAR(50) NOT NULL,
    process_type VARCHAR(20) NOT NULL,
    build_context TEXT NOT NULL,
    public_host VARCHAR(255) NULL,
    build_env JSONB NULL,
    image_id varchar(128) NULL,
    container_id varchar(128) NULL,
    internal_port INTEGER NULL,
    host_port INTEGER NULL,
    status VARCHAR(20) NOT NULL DEFAULT 'building',
    created_at TIMESTAMP NOT NULL DEFAULT now(),
    UNIQUE(application_id, name)
);

CREATE TABLE IF NOT EXISTS service_env_vars (
    service_id UUID NOT NULL REFERENCES services(id) ON DELETE CASCADE,
    key VARCHAR(255) NOT NULL,
    value TEXT NOT NULL,
    PRIMARY KEY (service_id, key)
);

CREATE TABLE IF NOT EXISTS project_env_vars (
    project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    key VARCHAR(255) NOT NULL,
    value TEXT NOT NULL,
    PRIMARY KEY (project_id, key)
);

CREATE TABLE IF NOT EXISTS application_env_vars (
    application_id UUID NOT NULL REFERENCES applications(id) ON DELETE CASCADE,
    key VARCHAR(255) NOT NULL,
    value TEXT NOT NULL,
    PRIMARY KEY (application_id, key)
);

END;
