const express = require('express');
const axios = require('axios');
const { OrderService } = require('./service');

const router = express.Router();
const service = new OrderService();

router.post('/orders', async (req, res) => {
  const saved = await service.createOrder(req.body);
  await notifyDownstream(saved);
  res.json({ id: saved.id, totalCents: saved.totalCents });
});

async function notifyDownstream(order) {
  return axios.post('https://hooks.example.com/orders', order);
}

module.exports = { router };
