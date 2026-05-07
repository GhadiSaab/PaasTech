BEGIN;

CREATE TABLE IF NOT EXISTS applications (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name VARCHAR(50) NOT NULL UNIQUE,
    image_id varchar(64) NULL,
    container_id varchar(64) NULL,
    port INTEGER NULL,
    status VARCHAR(20) NOT NULL DEFAULT 'stopped',
    env JSONB NULL,
    created_at TIMESTAMP NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS services (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name VARCHAR(50) NOT NULL,
    version VARCHAR(12) NOT NULL,
    image_id varchar(64) NULL,
    container_id varchar(64) NULL,
    port INTEGER NULL,
    created_at TIMESTAMP NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS application_services (
    application_id UUID NOT NULL,
    service_id UUID NOT NULL,
    PRIMARY KEY (application_id, service_id),
    FOREIGN KEY (application_id) REFERENCES applications(id) ON DELETE CASCADE,
    FOREIGN KEY (service_id) REFERENCES services(id) ON DELETE CASCADE
);

END;