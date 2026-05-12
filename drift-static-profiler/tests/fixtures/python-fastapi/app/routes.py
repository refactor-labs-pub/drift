from fastapi import APIRouter, Depends

from .db import get_session
from .repositories import OrderRepository
from .services import OrderService

router = APIRouter(prefix="/orders")


@router.post("")
def create_order(payload: dict, session=Depends(get_session)):
    repository = OrderRepository(session)
    service = OrderService(repository)
    order = service.create_order(payload)
    return {"id": order.id, "total_cents": order.total_cents}
