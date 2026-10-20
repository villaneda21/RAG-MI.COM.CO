-- Centro de ayuda: clientes, tickets, mensajes, snippets y reglas del bot.

CREATE EXTENSION IF NOT EXISTS pgcrypto;

CREATE TABLE customers (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name TEXT NOT NULL,
    domain TEXT NOT NULL DEFAULT '',
    email_micomco TEXT NOT NULL DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX customers_email_idx ON customers (lower(email_micomco));

CREATE TABLE departments (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name TEXT NOT NULL UNIQUE
);

CREATE TABLE agents (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name TEXT NOT NULL,
    email TEXT NOT NULL UNIQUE,
    department_id UUID NOT NULL REFERENCES departments (id),
    is_admin BOOLEAN NOT NULL DEFAULT FALSE,
    is_active BOOLEAN NOT NULL DEFAULT TRUE
);

CREATE TABLE tickets (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    customer_id UUID NOT NULL REFERENCES customers (id),
    assigned_agent_id UUID REFERENCES agents (id),
    department_id UUID NOT NULL REFERENCES departments (id),
    status TEXT NOT NULL CHECK (
        status IN (
            'nuevo',
            'en_bot',
            'esperando_agente',
            'en_proceso',
            'resuelto',
            'cerrado'
        )
    ),
    priority TEXT NOT NULL DEFAULT 'normal',
    auto_timeout_enabled BOOLEAN NOT NULL DEFAULT TRUE,
    last_client_message_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    warning_sent_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX tickets_status_idx ON tickets (status);
CREATE INDEX tickets_last_client_idx ON tickets (last_client_message_at);
CREATE INDEX tickets_department_idx ON tickets (department_id);

CREATE TABLE messages (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    ticket_id UUID NOT NULL REFERENCES tickets (id) ON DELETE CASCADE,
    sender_type TEXT NOT NULL CHECK (sender_type IN ('client', 'agent', 'bot')),
    content TEXT NOT NULL,
    media_type TEXT NOT NULL DEFAULT 'text' CHECK (media_type IN ('text', 'audio')),
    media_url TEXT,
    is_internal_note BOOLEAN NOT NULL DEFAULT FALSE,
    author_name TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX messages_ticket_created_idx ON messages (ticket_id, created_at);

CREATE TABLE canned_responses (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    shortcut TEXT NOT NULL UNIQUE,
    title TEXT NOT NULL,
    content TEXT NOT NULL,
    department_id UUID REFERENCES departments (id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE bot_rules (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    rule_key TEXT NOT NULL UNIQUE,
    rule_value JSONB NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE ticket_audit_logs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    ticket_id UUID NOT NULL REFERENCES tickets (id) ON DELETE CASCADE,
    agent_id UUID REFERENCES agents (id),
    action_type TEXT NOT NULL CHECK (
        action_type IN (
            'transfer',
            'status_change',
            'note_added',
            'auto_timeout',
            'bot_validation'
        )
    ),
    details TEXT NOT NULL DEFAULT '',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX ticket_audit_ticket_idx ON ticket_audit_logs (ticket_id, created_at);

INSERT INTO departments (id, name) VALUES
    ('11111111-1111-4111-8111-111111111111', 'Soporte Técnico'),
    ('22222222-2222-4222-8222-222222222222', 'Ventas'),
    ('33333333-3333-4333-8333-333333333333', 'Facturación');

INSERT INTO agents (id, name, email, department_id, is_admin, is_active) VALUES
    (
        'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa1',
        'Agente Soporte',
        'agente.soporte@mi.com.co',
        '11111111-1111-4111-8111-111111111111',
        FALSE,
        TRUE
    ),
    (
        'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa2',
        'Agente Ventas',
        'agente.ventas@mi.com.co',
        '22222222-2222-4222-8222-222222222222',
        FALSE,
        TRUE
    ),
    (
        'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa3',
        'Agente Facturación',
        'agente.facturacion@mi.com.co',
        '33333333-3333-4333-8333-333333333333',
        FALSE,
        TRUE
    ),
    (
        'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa4',
        'Administrador',
        'admin@mi.com.co',
        '11111111-1111-4111-8111-111111111111',
        TRUE,
        TRUE
    );

INSERT INTO canned_responses (shortcut, title, content, department_id) VALUES
    (
        '/dns',
        'Propagación DNS',
        'Los cambios de DNS pueden tardar entre 2 y 24 horas en propagarse. Revisa el estado en el panel, dentro de la zona DNS del dominio.',
        '11111111-1111-4111-8111-111111111111'
    ),
    (
        '/cpanel',
        'Acceso a cPanel',
        'Entra a Mi cuenta, abre Mis servicios y pulsa Acceder a cPanel. Si no ves el botón, confirma que el hosting esté activo.',
        '11111111-1111-4111-8111-111111111111'
    ),
    (
        '/precio',
        'Cotización',
        'Con gusto preparamos la cotización. Confírmanos el producto, el periodo (mensual o anual) y el dominio asociado.',
        '22222222-2222-4222-8222-222222222222'
    ),
    (
        '/factura',
        'Datos de facturación',
        'Para emitir o corregir la factura necesitamos el NIT, la razón social y el correo de facturación. Si ya los tienes, respóndenos por este chat.',
        '33333333-3333-4333-8333-333333333333'
    );

INSERT INTO bot_rules (rule_key, rule_value, description) VALUES
    (
        'inactivity_warning_minutes',
        '10'::jsonb,
        'Minutos de silencio del cliente antes de la advertencia.'
    ),
    (
        'inactivity_close_minutes',
        '5'::jsonb,
        'Minutos extra, tras la advertencia, antes del cierre automático.'
    ),
    (
        'warning_message_template',
        to_jsonb('Seguimos por aquí. Si no recibimos respuesta en los próximos minutos, cerraremos esta conversación.'::text),
        'Mensaje automático de advertencia por inactividad.'
    ),
    (
        'close_message_template',
        to_jsonb('Cerramos este chat por inactividad. Cuando quieras, escríbenos de nuevo y te ayudamos.'::text),
        'Mensaje automático de cierre por inactividad.'
    ),
    (
        'auto_close_enabled',
        'true'::jsonb,
        'Activa o pausa el cierre automático en todo el centro de ayuda.'
    );
