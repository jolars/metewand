import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

const root = fileURLToPath(new URL("../", import.meta.url));

test("shell installer forwards arguments, propagates errors, and cleans up", { skip: process.platform === "win32" }, (t) => {
  const directory = mkdtempSync(join(tmpdir(), "metewand-installer-test-"));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  writeFileSync(join(directory, "curl"), `#!/bin/sh
for argument do
  case "$argument" in
    https:*) test "$argument" = 'https://github.com/jolars/metewand/releases/latest/download/metewand-cli-installer.sh' || exit 99 ;;
  esac
  output=$argument
done
printf '%s\\n' 'printf "%s\\n" "$@"' 'exit 23' > "$output"
exit "$DOWNLOAD_STATUS"
`, { mode: 0o755 });

  for (const status of [0, 22]) {
    const result = spawnSync("sh", ["public/install", "--help", "two words"], {
      cwd: root,
      env: { ...process.env, PATH: `${directory}:${process.env.PATH}`, TMPDIR: directory, DOWNLOAD_STATUS: String(status) },
      encoding: "utf8",
    });
    assert.ifError(result.error);
    assert.equal(result.status, status || 23, result.stderr);
    assert.equal(result.stdout, status ? "" : "--help\ntwo words\n");
    assert.deepEqual(readdirSync(directory), ["curl"]);
  }
});

const powershell = spawnSync("pwsh", ["-NoProfile", "-Command", "exit 0"]);
test("PowerShell installer forwards arguments and stops on failed downloads", { skip: powershell.error?.code === "ENOENT" }, () => {
  const result = spawnSync("pwsh", ["-NoProfile", "-Command", `
    function Invoke-RestMethod {
      param($Uri)
      if ($Uri -ne 'https://github.com/jolars/metewand/releases/latest/download/metewand-cli-installer.ps1') { throw 'Wrong URL' }
      return 'Write-Output ($args -join "|")'
    }
    & ./public/install.ps1 --help 'two words'
    function Invoke-RestMethod { throw 'Download failed' }
    try { & ./public/install.ps1; exit 99 } catch { Write-Output 'Stopped' }
  `], { cwd: root, encoding: "utf8" });
  assert.ifError(result.error);
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout.trim().replaceAll("\r", ""), "--help|two words\nStopped");
});
