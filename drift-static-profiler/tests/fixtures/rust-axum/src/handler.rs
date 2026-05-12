use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use serde::Deserialize;
use std::sync::Arc;

use crate::service::{OrdersService, ServiceError};

#[derive(Deserialize)]
pub struct CreateOrderDto {
    pub customer_email: String,
    pub total_cents: i64,
    pub currency: Option<String>,
}

pub async fn create_order(
    State(svc): State<Arc<OrdersService>>,
    Json(dto): Json<CreateOrderDto>,
) -> impl IntoResponse {
    match svc
        .create_order(dto.customer_email, dto.total_cents, dto.currency)
        .await
    {
        Ok(id) => (StatusCode::CREATED, Json(serde_json::json!({ "id": id }))).into_response(),
        Err(ServiceError::BadRequest(msg)) => (StatusCode::BAD_REQUEST, msg).into_response(),
        Err(ServiceError::Persistence(_)) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
