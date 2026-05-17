-- Validates the trust-contract architecture (Stage 2/3): a `.sql` file
-- that parses cleanly but matches zero rules must still appear in the
-- report as a synthetic CallTreeNode with `findings: []`. Without this
-- the user can't tell whether drift scanned the file at all.

SELECT id, name, email FROM users WHERE id = 1;

INSERT INTO audit_log (event, user_id) VALUES ('login', 1);
