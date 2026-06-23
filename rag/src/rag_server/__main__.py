"""模块入口：支持 `python -m rag_server` 与 PyInstaller 入口复用。"""

from .main import main

if __name__ == "__main__":
    main()
