from sqlalchemy import Column, Integer, String, Float
from sqlalchemy.orm import declarative_base

Base = declarative_base()


class Order(Base):
    __tablename__ = "orders"
    id = Column(Integer, primary_key=True, index=True)
    customer_email = Column(String, nullable=False)
    total_cents = Column(Integer, nullable=False)
    currency = Column(String, default="USD")
