package handlers

import (
	"encoding/json"
	"net/http"

	"example.com/orders/service"
)

type OrdersHandler struct {
	svc *service.OrdersService
}

func NewOrdersHandler(s *service.OrdersService) *OrdersHandler {
	return &OrdersHandler{svc: s}
}

type createOrderDTO struct {
	CustomerEmail string `json:"customerEmail"`
	TotalCents    int64  `json:"totalCents"`
	Currency      string `json:"currency"`
}

func (h *OrdersHandler) CreateOrder(w http.ResponseWriter, r *http.Request) {
	var dto createOrderDTO
	if err := json.NewDecoder(r.Body).Decode(&dto); err != nil {
		http.Error(w, err.Error(), http.StatusBadRequest)
		return
	}
	order, err := h.svc.CreateOrder(dto.CustomerEmail, dto.TotalCents, dto.Currency)
	if err != nil {
		http.Error(w, err.Error(), http.StatusBadRequest)
		return
	}
	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(order)
}
