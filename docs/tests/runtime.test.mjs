import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

test("an unavailable workerd fails promptly without leaving a dev server running", () => {
  const env = {
    ...process.env,
    MINIFLARE_WORKERD_PATH: join(tmpdir(), `metewand-missing-workerd-${process.pid}`, "workerd"),
  };
  // The subprocess needs its own test runner rather than its parent's IPC context.
  delete env.NODE_TEST_CONTEXT;
  const result = spawnSync(process.execPath, ["--test", "tests/markdown.test.mjs"], {
    encoding: "utf8",
    env,
    timeout: 10_000,
  });
  assert.ifError(result.error);
  assert.equal(result.signal, null);
  assert.notEqual(result.status, 0);
  assert.match(result.stdout + result.stderr, /Cannot start workerd/);
  assert.match(result.stdout + result.stderr, /MINIFLARE_WORKERD_PATH/);
});
