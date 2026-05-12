package services

import scala.concurrent.{ExecutionContext, Future}
import repos.{Order, OrdersRepository}

class OrdersService(repo: OrdersRepository)(implicit ec: ExecutionContext) {

  def createOrder(email: String, totalCents: Long, currency: Option[String]): Future[Long] = {
    val order = buildOrder(email, totalCents, currency)
    validate(order)
    repo.save(order)
  }

  def buildOrder(email: String, totalCents: Long, currency: Option[String]): Order = {
    Order(id = 0L, customerEmail = email, totalCents = totalCents, currency = currency.getOrElse("USD"))
  }

  def validate(order: Order): Unit = {
    if (order.totalCents <= 0L) {
      throw new IllegalArgumentException("totalCents must be positive")
    }
    if (!order.customerEmail.contains("@")) {
      throw new IllegalArgumentException("invalid customerEmail")
    }
  }
}
