/// <reference types="vite/client" />

// 构建期注入的环境变量目前为空；新增时在 ImportMetaEnv 上声明。

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
