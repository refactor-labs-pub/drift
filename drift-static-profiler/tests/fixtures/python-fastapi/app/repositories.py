from .models import Order


class OrderRepository:
    def __init__(self, session):
        self.session = session

    def save(self, order: Order) -> Order:
        self.session.add(order)
        self.session.commit()
        self.session.refresh(order)
        return order

    def find_by_id(self, order_id: int):
        return self.session.query(Order).filter(Order.id == order_id).first()
