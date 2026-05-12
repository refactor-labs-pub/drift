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
];
