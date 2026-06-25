"""加载器工厂。"""

from __future__ import annotations

from pathlib import Path

from langchain_core.documents import Document

from ..config import OcrConfig
from .rich import DocxFileLoader, ImageFileLoader, PdfFileLoader, PptxFileLoader, file_mtime
from .simple import CsvFileLoader, HtmlFileLoader, TextFileLoader, XlsxFileLoader

SUPPORTED_EXTENSIONS = {
    ".pdf",
    ".docx",
    ".pptx",
    ".md",
    ".markdown",
    ".txt",
    ".html",
    ".htm",
    ".csv",
    ".xlsx",
    ".png",
    ".jpg",
    ".jpeg",
    ".webp",
    ".bmp",
}


def load_documents(
    file_path: str,
    *,
    project_id: str,
    project_path: str,
    ocr: OcrConfig,
) -> list[Document]:
    """按扩展名加载文档，统一补齐基础 metadata。"""
    path = Path(file_path).resolve()
    project_root = Path(project_path).resolve()
    extension = path.suffix.lower()
    if extension not in SUPPORTED_EXTENSIONS:
        raise ValueError(f"不支持的文件类型：{extension or '<none>'}")

    metadata = {
        "projectId": project_id,
        "source": str(path),
        "relativePath": str(path.relative_to(project_root)),
        "fileType": extension.lstrip("."),
        "mtime": file_mtime(str(path)),
    }
    loader = _create_loader(extension, str(path), metadata, ocr)
    docs = [doc for doc in loader.lazy_load() if doc.page_content.strip()]
    if not docs:
        raise ValueError("未提取到可索引文本")
    return docs


def _create_loader(extension: str, path: str, metadata: dict, ocr: OcrConfig):
    if extension == ".pdf":
        return PdfFileLoader(path, metadata, ocr)
    if extension == ".docx":
        return DocxFileLoader(path, metadata, ocr)
    if extension == ".pptx":
        return PptxFileLoader(path, metadata, ocr)
    if extension in {".md", ".markdown", ".txt"}:
        return TextFileLoader(path, metadata)
    if extension in {".html", ".htm"}:
        return HtmlFileLoader(path, metadata)
    if extension == ".csv":
        return CsvFileLoader(path, metadata)
    if extension == ".xlsx":
        return XlsxFileLoader(path, metadata)
    if extension in {".png", ".jpg", ".jpeg", ".webp", ".bmp"}:
        return ImageFileLoader(path, metadata, ocr)
    raise ValueError(f"不支持的文件类型：{extension or '<none>'}")
