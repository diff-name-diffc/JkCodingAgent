import { mkdir, rename } from "node:fs/promises";
import { arch, platform } from "node:process";
import { dirname, resolve } from "node:path";
import { spawn } from "node:child_process";

import { hostTargetTriple, resolveBuildTarget } from "./build-target.mjs";

const triple = process.env.TAURI_ENV_TARGET_TRIPLE || hostTargetTriple(platform, arch);
const { bunTarget, extension } = resolveBuildTarget(triple);

const root = resolve(import.meta.dirname, "../../..");
const temporary = resolve(root, `src-tauri/binaries/.pi-agent-sidecar-${triple}.tmp${extension}`);
const output = resolve(root, `src-tauri/binaries/pi-agent-sidecar-${triple}${extension}`);
await mkdir(dirname(output), { recursive: true });

await new Promise((resolvePromise, reject) => {
  const child = spawn(
    "bun",
    ["build", "--compile", `--target=${bunTarget}`, resolve(import.meta.dirname, "../src/index.ts"), "--outfile", temporary],
    { cwd: root, stdio: "inherit" }
  );
  child.on("error", reject);
  child.on("exit", (code) => code === 0 ? resolvePromise() : reject(new Error(`bun build 退出码 ${code}`)));
});
await rename(temporary, output);
console.log(`PI sidecar: ${output}`);
