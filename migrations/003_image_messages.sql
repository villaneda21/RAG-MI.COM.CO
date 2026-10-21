-- Imágenes estáticas en el chat. Los GIF siguen fuera del canal.

DO $$
DECLARE
    constraint_name text;
BEGIN
    SELECT con.conname
    INTO constraint_name
    FROM pg_constraint con
    JOIN pg_class rel ON rel.oid = con.conrelid
    WHERE rel.relname = 'messages'
      AND con.contype = 'c'
      AND pg_get_constraintdef(con.oid) ILIKE '%media_type%';
    IF constraint_name IS NOT NULL THEN
        EXECUTE format('ALTER TABLE messages DROP CONSTRAINT %I', constraint_name);
    END IF;
END $$;

ALTER TABLE messages
    ADD CONSTRAINT messages_media_type_check
    CHECK (media_type IN ('text', 'audio', 'image'));
