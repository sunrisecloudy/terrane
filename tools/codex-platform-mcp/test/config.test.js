import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { DEFAULT_CONTROL_URL, controlTokenPath, readControlToken, resolveControlConfig } from "../src/config.js";

test("control config reads explicit environment token", () => {
  const config = resolveControlConfig({
    env: {
      PLATFORM_CONTROL_URL: "http://127.0.0.1:9999",
      PLATFORM_CONTROL_TOKEN: "env-token",
    },
  });

  assert.deepEqual(config, {
    controlUrl: "http://127.0.0.1:9999",
    controlToken: "env-token",
  });
});

test("control config reads token file and trims whitespace", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "platform-token-"));
  const tokenFile = path.join(dir, "control.token");
  fs.writeFileSync(tokenFile, "file-token\n", { mode: 0o600 });

  const config = resolveControlConfig({
    env: { PLATFORM_CONTROL_TOKEN_FILE: tokenFile },
  });

  assert.equal(config.controlUrl, DEFAULT_CONTROL_URL);
  assert.equal(config.controlToken, "file-token");
});

test("control token path follows documented platform locations", () => {
  assert.equal(
    controlTokenPath({ env: {}, platform: "darwin", homeDir: "/Users/me" }),
    path.join("/Users/me", "Library", "Application Support", "native-ai-webapp", "control.token"),
  );
  assert.equal(
    controlTokenPath({ env: { XDG_RUNTIME_DIR: "/run/user/501" }, platform: "linux", homeDir: "/home/me" }),
    path.join("/run/user/501", "native-ai-webapp", "control.token"),
  );
  assert.equal(
    controlTokenPath({ env: { LOCALAPPDATA: "C:\\Users\\me\\AppData\\Local" }, platform: "win32", homeDir: "C:\\Users\\me" }),
    path.join("C:\\Users\\me\\AppData\\Local", "native-ai-webapp", "control.token"),
  );
});

test("control token loading fails fast without a token source", () => {
  assert.throws(
    () => readControlToken({ env: {}, platform: "linux", homeDir: "/home/me" }),
    /PLATFORM_CONTROL_TOKEN_FILE or XDG_RUNTIME_DIR/,
  );
  assert.throws(
    () => readControlToken({ env: { PLATFORM_CONTROL_TOKEN_FILE: "/missing/control.token" } }),
    /Control token file not found/,
  );
});
