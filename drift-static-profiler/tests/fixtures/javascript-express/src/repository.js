const { Order } = require('./db');

class OrderRepository {
  async save(order) {
    return Order.create(order);
  }

  async findById(id) {
    return Order.findById(id);
  }
}

module.exports = { OrderRepository };
