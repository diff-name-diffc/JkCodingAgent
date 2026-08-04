import assert from "node:assert/strict";
import test from "node:test";

import { hostTargetTriple, resolveBuildTarget } from "./build-target.mjs";

const targets = {
  "aarch64-apple-darwin": ["bun-darwin-arm64", ""],
  "x86_64-apple-darwin": ["bun-darwin-x64", ""],
  "aarch64-pc-windows-msvc": ["bun-windows-arm64", ".exe"],
  "x86_64-pc-windows-msvc": ["bun-windows-x64", ".exe"],
  "aarch64-unknown-linux-gnu": ["bun-linux-arm64", ""],
  "x86_64-unknown-linux-gnu": ["bun-linux-x64", ""],
  "aarch64-unknown-linux-musl": ["bun-linux-arm64-musl", ""],
  "x86_64-unknown-linux-musl": ["bun-linux-x64-musl", ""],
};

test("maps every supported Rust triple to its Bun target", () => {
  for (const [triple, [bunTarget, extension]] of Object.entries(targets)) {
    assert.deepEqual(resolveBuildTarget(triple), { bunTarget, extension });
  }
});

test("derives native triples and rejects unknown targets loudly", () => {
  assert.equal(hostTargetTriple("darwin", "arm64"), "aarch64-apple-darwin");
  assert.equal(hostTargetTriple("win32", "arm64"), "aarch64-pc-windows-msvc");
  assert.throws(() => hostTargetTriple("freebsd", "x64"), /不支持/);
  assert.throws(() => resolveBuildTarget("i686-unknown-linux-gnu"), /不支持/);
});
