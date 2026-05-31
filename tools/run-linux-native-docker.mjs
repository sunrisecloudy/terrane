#!/usr/bin/env node
import { execFileSync } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

export const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
export const defaultImage = "native-ai-linux-smoke:local";
export const defaultPlatform = process.arch === "x64" ? "" : "linux/amd64";

export function linuxDockerCommands({
  rootDir = repoRoot,
  image = defaultImage,
  platform = defaultPlatform,
  dockerfile = path.join(rootDir, "native", "linux", "Dockerfile"),
} = {}) {
  const buildArgs = [
    "build",
    "-f",
    dockerfile,
    "-t",
    image,
  ];
  if (platform) {
    buildArgs.push("--platform", platform);
  }
  buildArgs.push(path.join(rootDir, "native", "linux"));

  const runArgs = [
    "run",
    "--rm",
    "--mount",
    `type=bind,source=${rootDir},target=/workspace,readonly`,
    "--workdir",
    "/workspace",
    "-e",
    "GSETTINGS_BACKEND=memory",
    "-e",
    "GTK_A11Y=none",
    "-e",
    "NATIVE_AI_LINUX_SMOKE_LAUNCH=1",
    "-e",
    "NO_AT_BRIDGE=1",
    "-e",
    "WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1",
    "-e",
    "WEBKIT_DISABLE_COMPOSITING_MODE=1",
  ];
  if (platform) {
    runArgs.push("--platform", platform);
  }
  runArgs.push(
    image,
    "node",
    "--test",
    "--no-warnings",
    "tools/reference-host/test/linux-native-build.test.js",
  );

  return { buildArgs, runArgs };
}

function parseArgs(argv) {
  const options = {
    image: defaultImage,
    platform: defaultPlatform,
    build: true,
    run: true,
    dryRun: false,
  };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--image") {
      options.image = argv[(index += 1)];
    } else if (arg === "--platform") {
      options.platform = argv[(index += 1)];
    } else if (arg === "--skip-build") {
      options.build = false;
    } else if (arg === "--build-only") {
      options.run = false;
    } else if (arg === "--dry-run") {
      options.dryRun = true;
    } else {
      throw new Error(`Unknown argument: ${arg}`);
    }
  }
  return options;
}

function runDocker(args) {
  execFileSync("docker", args, { stdio: "inherit" });
}

function main() {
  const options = parseArgs(process.argv.slice(2));
  const commands = linuxDockerCommands(options);
  if (options.dryRun) {
    console.log(JSON.stringify(commands, null, 2));
    return;
  }
  if (options.build) {
    runDocker(commands.buildArgs);
  }
  if (options.run) {
    runDocker(commands.runArgs);
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    main();
  } catch (error) {
    console.error(error.stack ?? error.message);
    process.exitCode = 1;
  }
}
