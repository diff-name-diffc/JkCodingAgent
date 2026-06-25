"""HTTP 路由集合。"""

from . import config as config_router
from . import health as health_router
from . import ingest as ingest_router
from . import tests as tests_router

__all__ = ["config_router", "health_router", "ingest_router", "tests_router"]
