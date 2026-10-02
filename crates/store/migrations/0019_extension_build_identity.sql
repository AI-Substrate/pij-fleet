-- Loaded extension identity is reported by the registering runtime, never inferred.
ALTER TABLE seats ADD COLUMN extension_build TEXT;
ALTER TABLE seats ADD COLUMN extension_path TEXT;
PRAGMA user_version = 19;
