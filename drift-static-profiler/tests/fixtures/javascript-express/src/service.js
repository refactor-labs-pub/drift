const { OrderRepository } = require('./repository');

class OrderService {
  constructor() {
    this.repository = new OrderRepository();
  }

  async createOrder(payload) {
    const order = this.buildOrder(payload);
    this.validate(order);
    return this.repository.save(order);
  }

  buildOrder(payload) {
    return {
      customerEmail: payload.customerEmail,
      totalCents: payload.totalCents,
      currency: payload.currency || 'USD',
    };
  }

  validate(order) {
    if (!order.totalCents || order.totalCents <= 0) {
      throw new Error('totalCents must be positive');
    }
    if (!order.customerEmail.includes('@')) {
      throw new Error('invalid customerEmail');
    }
  }
}

module.exports = { OrderService };
