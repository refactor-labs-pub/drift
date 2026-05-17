-- psql meta-commands should be stripped (turned into blank lines)
-- so line numbers stay aligned. The SELECT below still triggers SQL001
-- on its original line.
\timing
\d users
\set OUT '/tmp/dump.csv'

SELECT * FROM events;
