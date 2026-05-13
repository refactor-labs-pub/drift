package com.example.repos

import java.sql.Connection
import java.sql.Statement

data class Order(
    val id: Long,
    val customerEmail: String,
    val totalCents: Long,
    val currency: String,
)

class OrdersRepository(private val conn: Connection) {

    fun save(order: Order): Long {
        val stmt = conn.prepareStatement(
            "INSERT INTO orders (customer_email, total_cents, currency) " +
                "VALUES (?, ?, ?)",
            Statement.RETURN_GENERATED_KEYS,
        )
        stmt.setString(1, order.customerEmail)
        stmt.setLong(2, order.totalCents)
        stmt.setString(3, order.currency)
        stmt.executeUpdate()
        val keys = stmt.generatedKeys
        keys.next()
        return keys.getLong(1)
    }

    fun findById(id: Long): Order? {
        val stmt = conn.prepareStatement(
            "SELECT id, customer_email, total_cents, currency FROM orders WHERE id = ?"
        )
        stmt.setLong(1, id)
        val rs = stmt.executeQuery()
        if (!rs.next()) return null
        return Order(
            id = rs.getLong(1),
            customerEmail = rs.getString(2),
            totalCents = rs.getLong(3),
            currency = rs.getString(4),
        )
    }
}
