package com.example.handlers

import com.example.services.OrdersService

data class CreateOrderDto(
    val customerEmail: String,
    val totalCents: Long,
    val currency: String?,
)

class OrdersHandler(private val svc: OrdersService) {

    fun createOrder(dto: CreateOrderDto): Long {
        return svc.createOrder(dto.customerEmail, dto.totalCents, dto.currency)
    }
}
