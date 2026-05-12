import 'reflect-metadata';
import { NestFactory } from '@nestjs/core';
import { AppModule } from './app.module';

async function bootstrap() {
  const app = await NestFactory.create(AppModule, { logger: ['error', 'warn'] });
  const port = Number(process.env.PORT || 3030);
  await app.listen(port);
  // eslint-disable-next-line no-console
  console.log(`orders-nestjs-fixture listening on http://localhost:${port}/orders`);
}

bootstrap();
