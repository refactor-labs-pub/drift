package service

import (
	"errors"
	"strings"

	"example.com/orders/repo"
)

type OrdersService struct {
	orders *repo.OrdersRepository
}

func NewOrdersService(r *repo.OrdersRepository) *OrdersService {
	return &OrdersService{orders: r}
}

func (s *OrdersService) CreateOrder(email string, totalCents int64, currency string) (*repo.Order, error) {
	order := s.buildOrder(email, totalCents, currency)
	if err := s.validate(order); err != nil {
		return nil, err
	}
	if err := s.orders.Save(order); err != nil {
		return nil, err
	}
	return order, nil
}

func (s *OrdersService) buildOrder(email string, totalCents int64, currency string) *repo.Order {
	if currency == "" {
		currency = "USD"
	}
	return &repo.Order{
		CustomerEmail: email,
		TotalCents:    totalCents,
		Currency:      currency,
	}
}

func (s *OrdersService) validate(o *repo.Order) error {
	if o.TotalCents <= 0 {
		return errors.New("totalCents must be positive")
	}
	if !strings.Contains(o.CustomerEmail, "@") {
		return errors.New("invalid customerEmail")
	}
	return nil
}
