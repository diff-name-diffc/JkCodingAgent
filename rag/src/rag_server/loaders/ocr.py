"""OCR 工厂。

RapidOCR 初始化开销较高，按 use_cuda 维度做进程内缓存。
"""

from __future__ import annotations

from functools import lru_cache
from typing import Any


@lru_cache(maxsize=2)
def get_ocr(use_cuda: bool = False) -> Any:
    """返回 RapidOCR 实例；依赖缺失时大声失败。"""
    from rapidocr import EngineType, RapidOCR

    params = {
        "Det.engine_type": EngineType.ONNXRUNTIME,
        "Cls.engine_type": EngineType.ONNXRUNTIME,
        "Rec.engine_type": EngineType.ONNXRUNTIME,
        "EngineConfig.onnxruntime.use_cuda": use_cuda,
    }
    return RapidOCR(params=params)


def ocr_text_from_result(result: Any) -> str:
    """兼容 RapidOCR v3 的输出对象。"""
    txts = getattr(result, "txts", None)
    return "\n".join(str(text) for text in txts) if txts else ""
