package com.example.services

import com.example.repos.Order
import com.example.repos.OrdersRepository

class OrdersService(private val repo: OrdersRepository) {

    fun createOrder(email: String, totalCents: Long, currency: String?): Long {
        val order = buildOrder(email, totalCents, currency)
        validate(order)
        return repo.save(order)
    }

    fun buildOrder(email: String, totalCents: Long, currency: String?): Order {
        return Order(
            id = 0L,
            customerEmail = email,
            totalCents = totalCents,
            currency = currency ?: "USD",
        )
    }

    private fun validate(order: Order) {
        if (order.totalCents <= 0L) {
            throw IllegalArgumentException("totalCents must be positive")
        }
        if (!order.customerEmail.contains("@")) {
            throw IllegalArgumentException("invalid customerEmail")
        }
    }
}
