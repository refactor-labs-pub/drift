import { Body, Controller, Post } from '@nestjs/common';
import { CreateOrderDto } from './create-order.dto';
import { OrdersService } from './orders.service';

@Controller('orders')
export class OrdersController {
  constructor(private readonly service: OrdersService) {}

  @Post()
  async create(@Body() dto: CreateOrderDto) {
    const saved = await this.service.createOrder(dto);
    return { id: saved.id, totalCents: saved.totalCents };
  }
}
