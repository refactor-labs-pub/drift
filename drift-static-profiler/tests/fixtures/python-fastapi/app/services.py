from .models import Order
from .repositories import OrderRepository


class OrderService:
    def __init__(self, repository: OrderRepository):
        self.repository = repository

    def create_order(self, payload: dict) -> Order:
        order = self.build_order(payload)
        self.validate(order)
        return self.repository.save(order)

    def build_order(self, payload: dict) -> Order:
        return Order(
            customer_email=payload["customer_email"],
            total_cents=payload["total_cents"],
            currency=payload.get("currency", "USD"),
        )

    def validate(self, order: Order) -> None:
        if order.total_cents <= 0:
            raise ValueError("total_cents must be positive")
        if "@" not in order.customer_email:
            raise ValueError("invalid customer_email")
