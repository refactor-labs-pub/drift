import type { FixtureSpec } from './types';

export const FIXTURES: FixtureSpec[] = [
  {
    key: 'python-fastapi',
    label: 'Python · FastAPI',
    json: '/fixtures/python-fastapi.json',
    description: 'POST /orders → service → repository → SQLAlchemy save',
  },
  {
    key: 'java-spring',
    label: 'Java · Spring Boot',
    json: '/fixtures/java-spring.json',
    description: 'POST /orders → @Service.createOrder → JpaRepository.save',
  },
  {
    key: 'typescript-nestjs',
    label: 'TypeScript · NestJS',
    json: '/fixtures/typescript-nestjs.json',
    description: 'POST /orders → @Injectable service → TypeORM repository.save',
  },
  {
    key: 'javascript-express',
    label: 'JavaScript · Express + Mongoose',
    json: '/fixtures/javascript-express.json',
    description: 'POST /orders → service → Mongoose model + axios webhook',
  },
  {
    key: 'go-gin',
    label: 'Go · net/http handler',
    json: '/fixtures/go-gin.json',
    description: 'POST /orders → service.CreateOrder → repository.Save',
  },
  {
    key: 'rust-axum',
    label: 'Rust · Axum + sqlx',
    json: '/fixtures/rust-axum.json',
    description: 'POST /orders → service.create_order → repo.save (sqlx::query_as)',
  },
  {
    key: 'scala-play',
    label: 'Scala · Play + Slick',
    json: '/fixtures/scala-play.json',
    description: 'POST /orders → OrdersService.createOrder → OrdersRepository.save (Slick db.run)',
  },
  {
    key: 'insights-demo',
    label: 'Insights Demo',
    json: '/fixtures/insights-demo.json',
    description: 'Synthetic Python file with N+1, blocking-in-async, and mutual recursion — to exercise the Insights + Scan Report pages',
  },
  {
    key: 'custom',
    label: 'Custom Scan',
    json: '/fixtures/custom.json',
    description: 'Output of `make scan /path` — auto-discovers roots if ENTRY is omitted; viewer auto-reloads when the file changes',
  },
];
