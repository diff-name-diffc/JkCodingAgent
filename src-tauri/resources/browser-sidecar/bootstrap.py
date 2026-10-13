"""在导入 browser-use 前关闭隐式网络服务及 stdout 日志。"""

import logging
import os
import sys
from importlib.metadata import PackageNotFoundError, version

_secrets = set()
EXPECTED_BROWSER_USE_VERSION = "0.13.11"


def verify_dependency():
    try:
        installed = version("browser-use")
    except PackageNotFoundError as error:
        raise RuntimeError(
            f"浏览器服务需要安装 browser-use=={EXPECTED_BROWSER_USE_VERSION}"
        ) from error
    if installed != EXPECTED_BROWSER_USE_VERSION:
        raise RuntimeError(
            f"浏览器服务需要 browser-use=={EXPECTED_BROWSER_USE_VERSION}，实际为 {installed}"
        )


def register_secret(secret):
    if secret:
        _secrets.add(str(secret))


def redact(text):
    for secret in tuple(_secrets):
        text = text.replace(secret, "[REDACTED]")
    return text


def redact_value(value):
    if isinstance(value, str):
        return redact(value)
    if isinstance(value, dict):
        return {redact(str(key)): redact_value(item) for key, item in value.items()}
    if isinstance(value, (list, tuple)):
        return [redact_value(item) for item in value]
    return value


class RedactingStream:
    def __init__(self, stream):
        self.stream = stream
        self.pending = ""

    def write(self, text):
        self.pending += text
        while "\n" in self.pending:
            line, self.pending = self.pending.split("\n", 1)
            self.stream.write(redact(line) + "\n")
        return len(text)

    def writelines(self, lines):
        # 显式走 write：经 __getattr__ 直连底层会绕过脱敏。
        for line in lines:
            self.write(line)

    def flush(self):
        if self.pending:
            # 部分写入后 flush 也不能把密钥前半段发出去，留到下次写入判定。
            withheld = max(
                (
                    length
                    for secret in tuple(_secrets)
                    for length in range(1, len(secret))
                    if self.pending.endswith(secret[:length])
                ),
                default=0,
            )
            ready = self.pending[:-withheld] if withheld else self.pending
            self.pending = self.pending[-withheld:] if withheld else ""
            self.stream.write(redact(ready))
        self.stream.flush()

    def __getattr__(self, name):
        return getattr(self.stream, name)


_original_stderr = sys.stderr
sys.stderr = RedactingStream(_original_stderr)
if sys.stdout is _original_stderr:
    sys.stdout = sys.stderr

for name, value in {
    "ANONYMIZED_TELEMETRY": "false",
    "BROWSER_USE_CLOUD_SYNC": "false",
    "BROWSER_USE_DISABLE_EXTENSIONS": "1",
    "BROWSER_USE_LOGGING_LEVEL": "error",
    "BROWSER_USE_SETUP_LOGGING": "false",
    "BROWSER_USE_VERSION_CHECK": "false",
}.items():
    os.environ[name] = value

verify_dependency()
logging.basicConfig(level=logging.ERROR)
