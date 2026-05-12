export class CreateOrderDto {
  customerEmail: string;
  totalCents: number;
  currency?: string;
}
