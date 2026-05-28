import { controlTokenPath, readControlToken } from "../../control-token.js";

export { controlTokenPath, readControlToken } from "../../control-token.js";

export const DEFAULT_CONTROL_URL = "http://127.0.0.1:7878";

export function resolveControlConfig({
  env = process.env,
  platform = process.platform,
  homeDir,
  existsSync,
  readFileSync,
} = {}) {
  const controlUrl = env.PLATFORM_CONTROL_URL ?? DEFAULT_CONTROL_URL;
  const controlToken = readControlToken({ env, platform, homeDir, existsSync, readFileSync });
  return { controlUrl, controlToken };
}
