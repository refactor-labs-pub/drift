import { Injectable, BadRequestException } from '@nestjs/common';
import { CreateOrderDto } from './create-order.dto';
import { Order } from './order.entity';
import { OrdersRepository } from './orders.repository';

@Injectable()
export class OrdersService {
  constructor(private readonly repository: OrdersRepository) {}

  async createOrder(dto: CreateOrderDto): Promise<Order> {
    const order = this.buildOrder(dto);
    this.validate(order);
    return this.repository.save(order);
  }

  private buildOrder(dto: CreateOrderDto): Order {
    const order = new Order();
    order.customerEmail = dto.customerEmail;
    order.totalCents = dto.totalCents;
    order.currency = dto.currency ?? 'USD';
    return order;
  }

  private validate(order: Order): void {
    if (!order.totalCents || order.totalCents <= 0) {
      throw new BadRequestException('totalCents must be positive');
    }
    if (!order.customerEmail.includes('@')) {
      throw new BadRequestException('invalid customerEmail');
    }
  }
}
