package repo

import (
	"database/sql"
	"fmt"
)

type Order struct {
	ID            int64
	CustomerEmail string
	TotalCents    int64
	Currency      string
}

type OrdersRepository struct {
	db *sql.DB
}

func NewOrdersRepository(db *sql.DB) *OrdersRepository {
	return &OrdersRepository{db: db}
}

func (r *OrdersRepository) Save(order *Order) error {
	_, err := r.db.Exec(
		"INSERT INTO orders (customer_email, total_cents, currency) VALUES ($1, $2, $3)",
		order.CustomerEmail, order.TotalCents, order.Currency,
	)
	if err != nil {
		return fmt.Errorf("save order: %w", err)
	}
	return nil
}

func (r *OrdersRepository) FindByID(id int64) (*Order, error) {
	row := r.db.QueryRow("SELECT id, customer_email, total_cents, currency FROM orders WHERE id = $1", id)
	o := &Order{}
	if err := row.Scan(&o.ID, &o.CustomerEmail, &o.TotalCents, &o.Currency); err != nil {
		return nil, err
	}
	return o, nil
}
