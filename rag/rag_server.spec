# -*- mode: python ; coding: utf-8 -*-
"""PyInstaller spec —— 把 rag_server 打包为 Tauri sidecar 单二进制。

产物名固定为 `rag-server`（见 pyproject.toml [tool.rag-server].binary_name），
打包脚本（scripts/build_sidecar.sh）会在此基础上追加 Tauri 要求的
target-triple 后缀并复制到 src-tauri/binaries/。

构建：
    cd rag
    uv run --with pyinstaller pyinstaller rag_server.spec --clean --noconfirm

注意：
  - onefile 模式首次启动有解压开销（约 200~500ms），可接受
  - hiddenimports 预留 uvicorn 子模块，避免运行时找不到 worker
  - datas 为空：FastAPI 不依赖额外数据文件
"""

from PyInstaller.utils.hooks import collect_submodules

block_cipher = None

hiddenimports = []
# uvicorn 的 worker 与协议实现按需 import，PyInstaller 静态分析会漏
hiddenimports += collect_submodules("uvicorn")
hiddenimports += collect_submodules("fastapi")
hiddenimports += collect_submodules("pydantic")


a = Analysis(
    ["src/rag_server/__main__.py"],
    pathex=["src"],
    binaries=[],
    datas=[],
    hiddenimports=hiddenimports,
    hookspath=[],
    hooksconfig={},
    runtime_hooks=[],
    excludes=[
        # 测试与类型检查依赖无需进产物
        "pytest",
        "ruff",
        "mypy",
    ],
    cipher=block_cipher,
    noarchive=False,
)

pyz = PYZ(a.pure, a.zipped_data, cipher=block_cipher)

exe = EXE(
    pyz,
    a.scripts,
    a.binaries,
    a.zipfiles,
    a.datas,
    [],
    name="rag-server",
    debug=False,
    bootloader_ignore_signals=False,
    strip=False,
    upx=True,
    runtime_tmpdir=None,
    console=True,  # sidecar 需要保留 stdout/stderr，必须为 console 应用
    disable_windowed_traceback=False,
    target_arch=None,
    codesign_identity=None,
    entitlements_file=None,
)
