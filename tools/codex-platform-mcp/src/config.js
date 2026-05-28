import fs from "node:fs";
import os from "node:os";
import path from "node:path";

export const DEFAULT_CONTROL_URL = "http://127.0.0.1:7878";

export function resolveControlConfig({
  env = process.env,
  platform = process.platform,
  homeDir = os.homedir(),
  existsSync = fs.existsSync,
  readFileSync = fs.readFileSync,
} = {}) {
  const controlUrl = env.PLATFORM_CONTROL_URL ?? DEFAULT_CONTROL_URL;
  const controlToken = readControlToken({ env, platform, homeDir, existsSync, readFileSync });
  return { controlUrl, controlToken };
}

export function readControlToken({
  env = process.env,
  platform = process.platform,
  homeDir = os.homedir(),
  existsSync = fs.existsSync,
  readFileSync = fs.readFileSync,
} = {}) {
  if (env.PLATFORM_CONTROL_TOKEN) {
    return env.PLATFORM_CONTROL_TOKEN;
  }

  const tokenPath = controlTokenPath({ env, platform, homeDir });
  if (!existsSync(tokenPath)) {
    throw new Error(`Control token file not found: ${tokenPath}`);
  }

  const token = readFileSync(tokenPath, "utf8").trim();
  if (!token) {
    throw new Error(`Control token file is empty: ${tokenPath}`);
  }
  return token;
}

export function controlTokenPath({ env = process.env, platform = process.platform, homeDir = os.homedir() } = {}) {
  if (env.PLATFORM_CONTROL_TOKEN_FILE) {
    return env.PLATFORM_CONTROL_TOKEN_FILE;
  }

  if (platform === "win32") {
    const localAppData = env.LOCALAPPDATA ?? path.join(homeDir, "AppData", "Local");
    return path.join(localAppData, "native-ai-webapp", "control.token");
  }

  if (platform === "darwin") {
    return path.join(homeDir, "Library", "Application Support", "native-ai-webapp", "control.token");
  }

  if (!env.XDG_RUNTIME_DIR) {
    throw new Error("Control token file requires PLATFORM_CONTROL_TOKEN_FILE or XDG_RUNTIME_DIR");
  }
  return path.join(env.XDG_RUNTIME_DIR, "native-ai-webapp", "control.token");
}
