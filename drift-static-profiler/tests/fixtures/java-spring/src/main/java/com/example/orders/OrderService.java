package com.example.orders;

import org.springframework.stereotype.Service;

@Service
public class OrderService {

    private final OrderRepository repository;

    public OrderService(OrderRepository repository) {
        this.repository = repository;
    }

    public Order createOrder(OrderRequest request) {
        Order order = buildOrder(request);
        validate(order);
        return repository.save(order);
    }

    private Order buildOrder(OrderRequest request) {
        Order order = new Order();
        order.setCustomerEmail(request.getCustomerEmail());
        order.setTotalCents(request.getTotalCents());
        if (request.getCurrency() != null) {
            order.setCurrency(request.getCurrency());
        }
        return order;
    }

    private void validate(Order order) {
        if (order.getTotalCents() == null || order.getTotalCents() <= 0) {
            throw new IllegalArgumentException("totalCents must be positive");
        }
        if (order.getCustomerEmail() == null || !order.getCustomerEmail().contains("@")) {
            throw new IllegalArgumentException("invalid customerEmail");
        }
    }
}
