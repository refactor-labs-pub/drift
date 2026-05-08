//! Stage 4 (sink) — persist a completed run + ranked issues to the local
//! SQLite database. The schema is created lazily here so this tool is safe
//! to call even if the agent never opened the runs UI.
//!
//! Existing `runs` table is owned by [`crate::db::init`]; this module adds a
//! sibling `run_issues` table keyed by `run_id`.

use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use super::analyze_samples::{Category, Severity};
use super::ToolManifest;
use crate::db;

pub const NAME: &str = "persist_run";
pub const DESCRIPTION: &str =
    "Save a completed profiling run and its ranked issues to the local SQLite store. Used at the \
     end of a workflow so the run shows up in 'Recent runs' and analysis can be re-loaded later.";
pub const PARAMETERS: &str = r#"{
  "type": "object",
  "properties": {
    "run_id": { "type": "string" },
    "project_path": { "type": "string" },
    "image_ref": { "type": "string" },
    "issues_found": { "type": "integer" },
    "critical_count": { "type": "integer" },
    "issues": {
      "type": "array",
      "items": {
        "type": "object",
        "properties": {
          "rank": { "type": "integer" },
          "function": { "type": "string" },
          "category": { "type": "string" },
          "severity": { "type": "string" },
          "self_pct": { "type": "number" },
          "total_pct": { "type": "number" },
          "samples": { "type": "integer" },
          "example_stack": { "type": "string" }
        },
        "required": ["rank", "function", "category", "severity", "self_pct", "total_pct"]
      }
    }
  },
  "required": ["run_id", "project_path", "issues_found", "critical_count"]
}"#;

#[derive(Debug, Deserialize)]
pub struct Args {
    pub run_id: String,
    pub project_path: String,
    pub image_ref: Option<String>,
    pub issues_found: u32,
    pub critical_count: u32,
    #[serde(default)]
    pub issues: Vec<IssueInput>,
}

#[derive(Debug, Deserialize)]
pub struct IssueInput {
    pub rank: u32,
    pub function: String,
    pub category: Category,
    pub severity: Severity,
    pub self_pct: f64,
    pub total_pct: f64,
    #[serde(default)]
    pub samples: u64,
    #[serde(default)]
    pub example_stack: String,
}

#[derive(Debug, Serialize)]
pub struct Output {
    pub run_id: String,
    pub issues_persisted: u32,
}

pub fn manifest() -> ToolManifest {
    ToolManifest {
        name: NAME,
        description: DESCRIPTION,
        parameters: PARAMETERS,
    }
}

pub async fn run(args: Args) -> Result<Output> {
    let pool = db::pool()
        .ok_or_else(|| anyhow!("db pool not initialised — db::init must run first"))?;
    persist_with_pool(pool, args).await
}

/// Pool-injected variant — used by `run` (which fetches the global pool) and
/// directly by tests (which build an in-memory pool). Creates `run_issues`
/// lazily so this module owns its own schema.
pub async fn persist_with_pool(pool: &SqlitePool, args: Args) -> Result<Output> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS run_issues (
            run_id TEXT NOT NULL,
            rank INTEGER NOT NULL,
            function TEXT NOT NULL,
            category TEXT NOT NULL,
            severity TEXT NOT NULL,
            self_pct REAL NOT NULL,
            total_pct REAL NOT NULL,
            samples INTEGER NOT NULL,
            example_stack TEXT NOT NULL,
            PRIMARY KEY (run_id, rank),
            FOREIGN KEY (run_id) REFERENCES runs(run_id) ON DELETE CASCADE
        );
        "#,
    )
    .execute(pool)
    .await
    .context("create run_issues table")?;

    let now = Utc::now().to_rfc3339();
    sqlx::query(
        r#"
        INSERT INTO runs (run_id, project_path, created_at, finished_at, issues_found, critical_count, error)
        VALUES (?, ?, ?, ?, ?, ?, NULL)
        ON CONFLICT(run_id) DO UPDATE SET
            project_path = excluded.project_path,
            finished_at = excluded.finished_at,
            issues_found = excluded.issues_found,
            critical_count = excluded.critical_count,
            error = NULL
        "#,
    )
    .bind(&args.run_id)
    .bind(&args.project_path)
    .bind(&now)
    .bind(&now)
    .bind(args.issues_found as i64)
    .bind(args.critical_count as i64)
    .execute(pool)
    .await
    .context("upsert run row")?;

    sqlx::query("DELETE FROM run_issues WHERE run_id = ?")
        .bind(&args.run_id)
        .execute(pool)
        .await
        .context("clear prior issues")?;

    let mut persisted = 0u32;
    for issue in &args.issues {
        sqlx::query(
            r#"
            INSERT INTO run_issues
                (run_id, rank, function, category, severity, self_pct, total_pct, samples, example_stack)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&args.run_id)
        .bind(issue.rank as i64)
        .bind(&issue.function)
        .bind(serde_json::to_string(&issue.category).unwrap_or_else(|_| "\"unknown\"".into()).trim_matches('"').to_string())
        .bind(serde_json::to_string(&issue.severity).unwrap_or_else(|_| "\"low\"".into()).trim_matches('"').to_string())
        .bind(issue.self_pct)
        .bind(issue.total_pct)
        .bind(issue.samples as i64)
        .bind(&issue.example_stack)
        .execute(pool)
        .await
        .context("insert run_issue")?;
        persisted += 1;
    }

    let _ = args.image_ref; // reserved for a future image_ref column

    Ok(Output {
        run_id: args.run_id,
        issues_persisted: persisted,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use std::str::FromStr;

    /// Build an in-memory pool with the same `runs` schema `db::init` creates,
    /// so persist_with_pool sees a realistic environment.
    async fn test_pool() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:").unwrap();
        let pool = SqlitePoolOptions::new().max_connections(1).connect_with(opts).await.unwrap();
        sqlx::query(
            r#"CREATE TABLE runs (
                run_id TEXT PRIMARY KEY,
                project_path TEXT NOT NULL,
                created_at TEXT NOT NULL,
                finished_at TEXT,
                issues_found INTEGER,
                critical_count INTEGER,
                error TEXT
            );"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    fn issue(rank: u32, function: &str, category: Category, severity: Severity) -> IssueInput {
        IssueInput {
            rank,
            function: function.into(),
            category,
            severity,
            self_pct: 50.0,
            total_pct: 60.0,
            samples: 100,
            example_stack: "main;handle".into(),
        }
    }

    #[tokio::test]
    async fn persists_run_and_issues() {
        let pool = test_pool().await;
        let out = persist_with_pool(
            &pool,
            Args {
                run_id: "r1".into(),
                project_path: "/p".into(),
                image_ref: Some("svc:1".into()),
                issues_found: 2,
                critical_count: 1,
                issues: vec![
                    issue(1, "psycopg.execute", Category::Database, Severity::Critical),
                    issue(2, "render", Category::Cpu, Severity::Medium),
                ],
            },
        )
        .await
        .unwrap();

        assert_eq!(out.issues_persisted, 2);

        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM run_issues WHERE run_id = ?")
            .bind("r1")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count.0, 2);

        let row: (String, String) = sqlx::query_as(
            "SELECT category, severity FROM run_issues WHERE run_id = 'r1' AND rank = 1",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0, "database");
        assert_eq!(row.1, "critical");
    }

    #[tokio::test]
    async fn rerun_replaces_prior_issues_not_accumulates() {
        let pool = test_pool().await;
        let mk = |issues: Vec<IssueInput>| Args {
            run_id: "r1".into(),
            project_path: "/p".into(),
            image_ref: None,
            issues_found: issues.len() as u32,
            critical_count: 0,
            issues,
        };
        persist_with_pool(&pool, mk(vec![issue(1, "a", Category::Cpu, Severity::Low)]))
            .await
            .unwrap();
        persist_with_pool(
            &pool,
            mk(vec![
                issue(1, "b", Category::Cpu, Severity::Low),
                issue(2, "c", Category::Cpu, Severity::Low),
            ]),
        )
        .await
        .unwrap();

        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM run_issues WHERE run_id = 'r1'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count.0, 2, "old rank=1 'a' should have been deleted");

        let leaf: (String,) =
            sqlx::query_as("SELECT function FROM run_issues WHERE run_id = 'r1' AND rank = 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(leaf.0, "b");
    }

    #[tokio::test]
    async fn upserts_run_row_on_repeat() {
        let pool = test_pool().await;
        let mk = |found: u32, crit: u32| Args {
            run_id: "r1".into(),
            project_path: "/p".into(),
            image_ref: None,
            issues_found: found,
            critical_count: crit,
            issues: vec![],
        };
        persist_with_pool(&pool, mk(3, 1)).await.unwrap();
        persist_with_pool(&pool, mk(7, 2)).await.unwrap();

        let row: (i64, i64) =
            sqlx::query_as("SELECT issues_found, critical_count FROM runs WHERE run_id = 'r1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(row, (7, 2));
    }

    #[tokio::test]
    async fn run_without_initialised_pool_errors() {
        // db::pool() is the singleton; in this binary's test process it has
        // not been initialised, so run() should return the documented error.
        let err = run(Args {
            run_id: "x".into(),
            project_path: "/p".into(),
            image_ref: None,
            issues_found: 0,
            critical_count: 0,
            issues: vec![],
        })
        .await
        .unwrap_err();
        assert!(err.to_string().contains("db pool not initialised"));
    }
}
