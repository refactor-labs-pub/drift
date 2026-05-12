package controllers

import scala.concurrent.{ExecutionContext, Future}
import services.OrdersService

case class CreateOrderDto(customerEmail: String, totalCents: Long, currency: Option[String])

class OrdersController(svc: OrdersService)(implicit ec: ExecutionContext) {

  def createOrder(dto: CreateOrderDto): Future[Long] = {
    svc.createOrder(dto.customerEmail, dto.totalCents, dto.currency)
  }
}
