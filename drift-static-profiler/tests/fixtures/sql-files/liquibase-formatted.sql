-- Liquibase formatted SQL: changeset directives are stripped,
-- the underlying SQL still gets linted.
--
-- Line 7: SQL001 (SELECT *)

--liquibase formatted sql
--changeset alice:add-users-view
SELECT * FROM users WHERE active = true;
--rollback DROP VIEW users_view;
