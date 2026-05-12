import { Injectable } from '@nestjs/common';
import { InjectRepository } from '@nestjs/typeorm';
import { Repository } from 'typeorm';
import { Order } from './order.entity';

@Injectable()
export class OrdersRepository {
  constructor(
    @InjectRepository(Order)
    private readonly repo: Repository<Order>,
  ) {}

  async save(order: Order): Promise<Order> {
    return this.repo.save(order);
  }

  async findById(id: number): Promise<Order | null> {
    return this.repo.findOneBy({ id });
  }
}
