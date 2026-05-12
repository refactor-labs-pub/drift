import { Column, Entity, PrimaryGeneratedColumn } from 'typeorm';

@Entity('orders')
export class Order {
  @PrimaryGeneratedColumn()
  id: number;

  @Column()
  customerEmail: string;

  @Column()
  totalCents: number;

  @Column({ default: 'USD' })
  currency: string;
}
