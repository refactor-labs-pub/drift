package repos

import scala.concurrent.Future
import slick.jdbc.PostgresProfile.api._

case class Order(id: Long, customerEmail: String, totalCents: Long, currency: String)

class OrdersRepository(db: Database) {

  def save(order: Order): Future[Long] = {
    val q = sqlu"""INSERT INTO orders (customer_email, total_cents, currency)
                   VALUES (${order.customerEmail}, ${order.totalCents}, ${order.currency})"""
    db.run(q).map(_.toLong)(scala.concurrent.ExecutionContext.global)
  }

  def findById(id: Long): Future[Option[Order]] = {
    val q = sql"""SELECT id, customer_email, total_cents, currency
                  FROM orders WHERE id = $id""".as[(Long, String, Long, String)]
    db.run(q.headOption).map(_.map(t => Order(t._1, t._2, t._3, t._4)))(scala.concurrent.ExecutionContext.global)
  }
}
