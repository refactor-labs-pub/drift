const mongoose = require('mongoose');

const orderSchema = new mongoose.Schema({
  customerEmail: { type: String, required: true },
  totalCents: { type: Number, required: true },
  currency: { type: String, default: 'USD' },
});

const Order = mongoose.model('Order', orderSchema);

module.exports = { Order, mongoose };
