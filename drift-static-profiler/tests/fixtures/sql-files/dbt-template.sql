-- dbt model file with Jinja templates — drift skips this whole file
-- because rendering requires `dbt compile`. No findings expected.

{{ config(materialized='table') }}

SELECT * FROM {{ ref('stg_users') }}
WHERE created_at > '{{ var("min_date") }}'
