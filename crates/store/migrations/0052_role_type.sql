-- PRD 006 s03 (plan 237, claimed by pm-001 for #344): a role document's seat
-- type, beside its pointer. Written whenever pij commits the role document
-- (add, edit, retype) and recomputed by the rebuild from the branch head, so
-- `pij list` reads a seat's type with no governance read. NULL for every
-- other document and for a role written before types. Additive: a binary at
-- 51 never reads the column; going back means a daemon at 51, which refuses
-- this store as newer, so the down path is a store restore (or dropping the
-- column, which loses only what the rebuild recomputes).
ALTER TABLE community_pointers ADD COLUMN role_type TEXT;

PRAGMA user_version = 52;
