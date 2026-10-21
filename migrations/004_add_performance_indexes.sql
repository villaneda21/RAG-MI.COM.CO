CREATE INDEX IF NOT EXISTS idx_messages_ticket_created ON messages(ticket_id, created_at);
CREATE INDEX IF NOT EXISTS idx_tickets_status_dept ON tickets(status, department_id);
CREATE INDEX IF NOT EXISTS idx_tickets_customer ON tickets(customer_id);
