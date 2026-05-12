use crate::ImportRecord;
use serde::{Deserialize, Serialize};

/// Resource category for a static call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    Db,
    Network,
    Io,
    Cache,
    Queue,
    Log,
    Compute,
}

impl Category {
    pub const ALL: &'static [Category] = &[
        Category::Db,
        Category::Network,
        Category::Io,
        Category::Cache,
        Category::Queue,
        Category::Log,
        Category::Compute,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            Category::Db => "db",
            Category::Network => "network",
            Category::Io => "io",
            Category::Cache => "cache",
            Category::Queue => "queue",
            Category::Log => "log",
            Category::Compute => "compute",
        }
    }
}

/// Result of classifying a single call site, with the evidence used.
/// Useful for the UI ("we said DB because the receiver `session` was bound to
/// `sqlalchemy.orm.Session`").
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Classification {
    pub category: Category,
    pub tier: ClassifyTier,
    pub evidence: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClassifyTier {
    /// Tier B: receiver is a directly-imported module from a catalogued lib
    ImportedModule,
    /// Tier C: receiver name matches a well-known pattern (e.g. `session`, `axios`)
    ReceiverPattern,
    /// Tier D: method name is unambiguous in any context
    MethodSignature,
}

/// Classify a call site given its method name, optional receiver, and the file's imports.
///
/// Strategy (tier order):
///   B. If receiver is a name imported from a catalogued module → use that category
///   C. If receiver name matches a well-known receiver pattern → that category
///   D. If method name is unambiguous on its own → that category
///   Else → None (likely user code)
///
/// The "ambiguous-on-its-own" method names (`save`, `add`, `find`, `update`, ...)
/// are deliberately excluded from Tier D. They require Tier B or C evidence.
pub fn classify(
    name: &str,
    receiver: Option<&str>,
    imports: &[ImportRecord],
) -> Option<Classification> {
    // Tier B: receiver bound to an imported categorized module
    if let Some(r) = receiver {
        for imp in imports {
            if imp.local_name == r {
                if let Some(cat) = classify_module(&imp.module_path) {
                    return Some(Classification {
                        category: cat,
                        tier: ClassifyTier::ImportedModule,
                        evidence: format!("receiver `{}` imported from `{}`", r, imp.module_path),
                    });
                }
            }
        }
    }

    // Tier C: receiver name pattern
    if let Some(r) = receiver {
        if let Some(cat) = classify_receiver_pattern(r) {
            return Some(Classification {
                category: cat,
                tier: ClassifyTier::ReceiverPattern,
                evidence: format!("receiver name `{r}` matches {} pattern", cat.as_str()),
            });
        }
    }

    // Tier D: unambiguous method name
    if let Some(cat) = classify_unambiguous_method(name) {
        return Some(Classification {
            category: cat,
            tier: ClassifyTier::MethodSignature,
            evidence: format!("method `{name}` is an unambiguous {} call", cat.as_str()),
        });
    }

    None
}

/// CATALOG — module path → category.
/// Prefix-based: `sqlalchemy.orm` matches `sqlalchemy`, `org.springframework.data.jpa` matches `org.springframework.data`.
/// Sources: PyPI top ORMs/HTTP libs, npm Node.js ORMs/HTTP clients,
/// Maven Central Spring/JPA/Hibernate/JDBC packages.
pub fn classify_module(module: &str) -> Option<Category> {
    for (prefix, cat) in MODULE_CATALOG {
        if module == *prefix
            || module.starts_with(&format!("{prefix}."))
            || module.starts_with(&format!("{prefix}/"))
        {
            return Some(*cat);
        }
    }
    None
}

const MODULE_CATALOG: &[(&str, Category)] = &[
    // ─── PYTHON ──────────────────────────────────────────────────────────
    // ORM / DB
    ("sqlalchemy", Category::Db),
    ("psycopg2", Category::Db),
    ("psycopg", Category::Db),
    ("pymongo", Category::Db),
    ("motor", Category::Db),
    ("django.db", Category::Db),
    ("peewee", Category::Db),
    ("tortoise", Category::Db),
    ("sqlmodel", Category::Db),
    ("databases", Category::Db),
    ("asyncpg", Category::Db),
    ("aiomysql", Category::Db),
    ("aiosqlite", Category::Db),
    ("mysql.connector", Category::Db),
    ("sqlite3", Category::Db),
    ("prisma", Category::Db),
    ("alembic", Category::Db),
    ("redis", Category::Cache),
    ("aioredis", Category::Cache),
    ("memcache", Category::Cache),
    ("pymemcache", Category::Cache),
    ("elasticsearch", Category::Db),
    ("cassandra", Category::Db),
    // HTTP / Network
    ("requests", Category::Network),
    ("httpx", Category::Network),
    ("aiohttp", Category::Network),
    ("urllib", Category::Network),
    ("urllib3", Category::Network),
    ("httplib2", Category::Network),
    ("grpc", Category::Network),
    ("socket", Category::Network),
    ("websockets", Category::Network),
    ("websocket", Category::Network),
    // File / IO
    ("io", Category::Io),
    ("pathlib", Category::Io),
    ("os.path", Category::Io),
    ("shutil", Category::Io),
    ("aiofiles", Category::Io),
    // Queues / messaging
    ("kafka", Category::Queue),
    ("aiokafka", Category::Queue),
    ("confluent_kafka", Category::Queue),
    ("pika", Category::Queue),
    ("kombu", Category::Queue),
    ("celery", Category::Queue),
    ("boto3", Category::Network), // AWS SDK — mostly network
    ("botocore", Category::Network),
    // Logging
    ("logging", Category::Log),
    ("loguru", Category::Log),
    ("structlog", Category::Log),

    // ─── JAVA ────────────────────────────────────────────────────────────
    // JDBC / JPA / Hibernate
    ("java.sql", Category::Db),
    ("javax.sql", Category::Db),
    ("javax.persistence", Category::Db),
    ("jakarta.persistence", Category::Db),
    ("org.hibernate", Category::Db),
    ("org.springframework.data", Category::Db),
    ("org.springframework.jdbc", Category::Db),
    ("org.springframework.orm", Category::Db),
    ("org.springframework.transaction", Category::Db),
    ("org.mongodb", Category::Db),
    ("redis.clients.jedis", Category::Cache),
    ("io.lettuce", Category::Cache),
    // HTTP
    ("java.net.http", Category::Network),
    ("java.net", Category::Network),
    ("okhttp3", Category::Network),
    ("com.squareup.okhttp", Category::Network),
    ("org.apache.http", Category::Network),
    ("org.springframework.web.client", Category::Network),
    ("org.springframework.web.reactive.function.client", Category::Network),
    ("org.springframework.web", Category::Network),
    ("retrofit2", Category::Network),
    // File / IO
    ("java.io", Category::Io),
    ("java.nio", Category::Io),
    // Messaging
    ("org.apache.kafka", Category::Queue),
    ("org.springframework.kafka", Category::Queue),
    ("javax.jms", Category::Queue),
    ("jakarta.jms", Category::Queue),
    ("org.springframework.amqp", Category::Queue),
    ("software.amazon.awssdk.sqs", Category::Queue),
    // Logging
    ("org.slf4j", Category::Log),
    ("ch.qos.logback", Category::Log),
    ("org.apache.logging.log4j", Category::Log),
    ("java.util.logging", Category::Log),

    // ─── TYPESCRIPT / JAVASCRIPT (npm) ───────────────────────────────────
    // ORM / DB
    ("typeorm", Category::Db),
    ("@nestjs/typeorm", Category::Db),
    ("@mikro-orm", Category::Db),
    ("mikro-orm", Category::Db),
    ("prisma", Category::Db),
    ("@prisma/client", Category::Db),
    ("mongoose", Category::Db),
    ("@nestjs/mongoose", Category::Db),
    ("sequelize", Category::Db),
    ("@sequelize/core", Category::Db),
    ("knex", Category::Db),
    ("kysely", Category::Db),
    ("drizzle-orm", Category::Db),
    ("pg", Category::Db),
    ("postgres", Category::Db),
    ("mysql", Category::Db),
    ("mysql2", Category::Db),
    ("sqlite3", Category::Db),
    ("better-sqlite3", Category::Db),
    ("redis", Category::Cache),
    ("ioredis", Category::Cache),
    ("@upstash/redis", Category::Cache),
    ("memcached", Category::Cache),
    ("@elastic/elasticsearch", Category::Db),
    // HTTP / network
    ("axios", Category::Network),
    ("node-fetch", Category::Network),
    ("got", Category::Network),
    ("ky", Category::Network),
    ("undici", Category::Network),
    ("superagent", Category::Network),
    ("@nestjs/axios", Category::Network),
    ("http", Category::Network),
    ("https", Category::Network),
    ("ws", Category::Network),
    ("socket.io", Category::Network),
    ("socket.io-client", Category::Network),
    ("@grpc/grpc-js", Category::Network),
    ("aws-sdk", Category::Network),
    ("@aws-sdk", Category::Network),
    // File / IO
    ("fs", Category::Io),
    ("fs/promises", Category::Io),
    ("node:fs", Category::Io),
    ("node:fs/promises", Category::Io),
    ("path", Category::Io),
    ("stream", Category::Io),
    // Queues
    ("kafkajs", Category::Queue),
    ("@nestjs/microservices", Category::Queue),
    ("amqplib", Category::Queue),
    ("bullmq", Category::Queue),
    ("bull", Category::Queue),
    // Logging
    ("winston", Category::Log),
    ("pino", Category::Log),
    ("bunyan", Category::Log),
    ("@nestjs/common", Category::Log), // Logger lives here (best-effort)
];

/// Receiver-name patterns — strong hints even when type info is missing.
/// Example: `session.add(...)` → DB regardless of where `session` came from.
pub fn classify_receiver_pattern(receiver: &str) -> Option<Category> {
    let lower = receiver.to_ascii_lowercase();
    let r = lower.as_str();
    // Db
    if matches_any(r, &[
        "session", "db", "database", "engine", "conn", "connection",
        "tx", "transaction", "cursor", "stmt", "statement",
        "repo", "repository", "dao", "entitymanager", "em",
        "queryrunner", "querybuilder", "knex", "prisma",
        "mongo", "mongoose", "model",
    ]) {
        return Some(Category::Db);
    }
    // Network
    if matches_any(r, &[
        "axios", "http", "https", "fetcher", "httpclient", "restclient",
        "resttemplate", "webclient", "grpc", "client",
    ]) {
        return Some(Category::Network);
    }
    // Cache
    if matches_any(r, &["cache", "redis", "memcache", "memcached"]) {
        return Some(Category::Cache);
    }
    // Queue
    if matches_any(r, &["queue", "producer", "consumer", "kafka", "rabbit", "broker"]) {
        return Some(Category::Queue);
    }
    // Log
    if matches_any(r, &["log", "logger"]) {
        return Some(Category::Log);
    }
    None
}

fn matches_any(name: &str, patterns: &[&str]) -> bool {
    patterns.iter().any(|p| *p == name)
}

/// Unambiguous method names — these almost always indicate the category in any
/// language/framework. Deliberately tight; generic verbs like `save`, `add`,
/// `find`, `get`, `delete`, `update` are NOT here.
pub fn classify_unambiguous_method(name: &str) -> Option<Category> {
    // JDBC, raw SQL, Mongo
    const DB: &[&str] = &[
        "executeQuery", "executeUpdate", "prepareStatement", "prepareCall",
        "createQueryBuilder", "getRepository",
        "findOneAndUpdate", "findOneAndDelete", "findOneAndReplace",
        "findByIdAndUpdate", "findByIdAndDelete",
        "create_engine", "sessionmaker",
    ];
    if DB.iter().any(|n| *n == name) {
        return Some(Category::Db);
    }
    // HTTP — very specific verbs
    const NET: &[&str] = &["urlopen"];
    if NET.iter().any(|n| *n == name) {
        return Some(Category::Network);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn imp(local: &str, module: &str) -> ImportRecord {
        ImportRecord {
            local_name: local.into(),
            module_path: module.into(),
            imported_name: None,
            line: 0,
        }
    }

    #[test]
    fn imported_module_wins_tier_b() {
        let imports = vec![imp("requests", "requests")];
        let c = classify("get", Some("requests"), &imports).unwrap();
        assert_eq!(c.category, Category::Network);
        assert_eq!(c.tier, ClassifyTier::ImportedModule);
    }

    #[test]
    fn receiver_pattern_catches_session_add() {
        // No imports, just the receiver name
        let c = classify("add", Some("session"), &[]).unwrap();
        assert_eq!(c.category, Category::Db);
        assert_eq!(c.tier, ClassifyTier::ReceiverPattern);
    }

    #[test]
    fn unambiguous_method_classifies_jdbc() {
        let c = classify("executeQuery", None, &[]).unwrap();
        assert_eq!(c.category, Category::Db);
        assert_eq!(c.tier, ClassifyTier::MethodSignature);
    }

    #[test]
    fn no_false_positive_on_set_add() {
        // `seen.add(item)` — receiver "seen", method "add", no imports
        assert!(classify("add", Some("seen"), &[]).is_none());
    }

    #[test]
    fn no_false_positive_on_dict_update() {
        assert!(classify("update", Some("result"), &[]).is_none());
    }

    #[test]
    fn no_false_positive_on_list_find() {
        assert!(classify("find", Some("items"), &[]).is_none());
    }

    #[test]
    fn no_false_positive_on_save_to_disk() {
        // "save" alone, no receiver context, no imports → not categorized
        assert!(classify("save", None, &[]).is_none());
    }

    #[test]
    fn axios_get_classifies_network() {
        let imports = vec![imp("axios", "axios")];
        let c = classify("get", Some("axios"), &imports).unwrap();
        assert_eq!(c.category, Category::Network);
    }

    #[test]
    fn prisma_user_create_classifies_db() {
        // `prisma.user.create({...})` → receiver text is `user`, but we
        // catch it via the receiver-pattern bucket (`model`) — and if we
        // can see the import `prisma`, the receiver-pattern fallback isn't
        // even needed. Verify the pattern path:
        // The query gives us receiver="user" for `prisma.user.create(...)`.
        // `user` isn't a receiver pattern, but `prisma` IS. So if the
        // analyzer captures the chain we'd want `prisma`. Confirm both paths.
        // Path 1: receiver = "prisma" (direct call on the imported binding)
        let imports = vec![imp("prisma", "@prisma/client")];
        let c = classify("create", Some("prisma"), &imports).unwrap();
        assert_eq!(c.category, Category::Db);
        // Path 2: receiver = "user" — no Tier B match; receiver pattern doesn't include "user"
        assert!(classify("create", Some("user"), &imports).is_none());
    }
}
