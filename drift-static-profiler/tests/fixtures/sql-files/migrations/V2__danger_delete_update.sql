-- Two table-wipe hazards in one file, on known lines.
--
-- Line 6: SQL002 (DELETE without WHERE)
-- Line 8: SQL003 (UPDATE without WHERE)

DELETE FROM sessions;

UPDATE accounts SET active = false;
