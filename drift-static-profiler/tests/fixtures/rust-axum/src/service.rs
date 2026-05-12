use crate::repo::{Order, OrdersRepository};

pub struct OrdersService {
    repo: OrdersRepository,
}

impl OrdersService {
    pub fn new(repo: OrdersRepository) -> Self {
        Self { repo }
    }

    pub async fn create_order(
        &self,
        email: String,
        total_cents: i64,
        currency: Option<String>,
    ) -> Result<i64, ServiceError> {
        let order = self.build_order(email, total_cents, currency);
        self.validate(&order)?;
        self.repo
            .save(&order)
            .await
            .map_err(ServiceError::Persistence)
    }

    fn build_order(
        &self,
        email: String,
        total_cents: i64,
        currency: Option<String>,
    ) -> Order {
        Order {
            id: 0,
            customer_email: email,
            total_cents,
            currency: currency.unwrap_or_else(|| "USD".to_string()),
        }
    }

    fn validate(&self, o: &Order) -> Result<(), ServiceError> {
        if o.total_cents <= 0 {
            return Err(ServiceError::BadRequest("totalCents must be positive".into()));
        }
        if !o.customer_email.contains('@') {
            return Err(ServiceError::BadRequest("invalid customerEmail".into()));
        }
        Ok(())
    }
}

pub enum ServiceError {
    BadRequest(String),
    Persistence(sqlx::Error),
}
