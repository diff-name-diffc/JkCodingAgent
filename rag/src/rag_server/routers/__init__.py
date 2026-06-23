"""HTTP 路由集合。"""

from . import config as config_router
from . import health as health_router

__all__ = ["config_router", "health_router"]
