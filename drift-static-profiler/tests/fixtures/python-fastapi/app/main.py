from fastapi import FastAPI

from .routes import router

app = FastAPI(title="Orders demo")
app.include_router(router)
