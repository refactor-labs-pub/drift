-- Flyway-style migration. Two statements, both pattern-matched
-- against the v1 SQL rule catalog.
--
-- Statement 1: SQL001 (SELECT *)
-- Statement 2: SQL004 (INSERT no column list)

SELECT * FROM users;

INSERT INTO users VALUES (1, 'admin', 'admin@example.com');
