-- Validates that the MIG_* rules fire on real migration hazards.
-- Each statement is precisely one hazard so the test asserts rule IDs
-- by line.

-- Line 6: MIG_CREATE_INDEX_NOT_CONCURRENT
CREATE INDEX idx_users_email ON users (email);

-- Line 9: MIG_DROP_COLUMN
ALTER TABLE users DROP COLUMN deprecated_flag;

-- Line 12: MIG_ALTER_COLUMN_TYPE
ALTER TABLE users ALTER COLUMN id TYPE bigint;

-- Line 15: MIG_ADD_FK_NOT_VALID
ALTER TABLE orders ADD CONSTRAINT orders_user_fk FOREIGN KEY (user_id) REFERENCES users(id);

-- Line 18: MIG_ADD_COLUMN_NOT_NULL_NO_DEFAULT
ALTER TABLE users ADD COLUMN status text NOT NULL;

-- Line 21: MIG_DROP_TABLE
DROP TABLE legacy_sessions;
