ALTER TABLE attachments
ADD COLUMN stored_on_disk INTEGER NOT NULL DEFAULT 0 CHECK (stored_on_disk IN (0, 1));
