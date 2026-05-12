use sqlx::PgPool;

pub struct Order {
    pub id: i64,
    pub customer_email: String,
    pub total_cents: i64,
    pub currency: String,
}

pub struct OrdersRepository {
    pool: PgPool,
}

impl OrdersRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn save(&self, order: &Order) -> Result<i64, sqlx::Error> {
        let row: (i64,) = sqlx::query_as(
            "INSERT INTO orders (customer_email, total_cents, currency) \
             VALUES ($1, $2, $3) RETURNING id",
        )
        .bind(&order.customer_email)
        .bind(order.total_cents)
        .bind(&order.currency)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0)
    }

    pub async fn find_by_id(&self, id: i64) -> Result<Order, sqlx::Error> {
        let row: (i64, String, i64, String) = sqlx::query_as(
            "SELECT id, customer_email, total_cents, currency FROM orders WHERE id = $1",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await?;
        Ok(Order {
            id: row.0,
            customer_email: row.1,
            total_cents: row.2,
            currency: row.3,
        })
    }
}
