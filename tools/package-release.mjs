#!/usr/bin/env node
import { execFileSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

export const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

const FIXED_DOS_TIME = 0;
const FIXED_DOS_DATE = 33;
const PLATFORM_VERSION = "0.1.0";
const ZIG_CORE_TARGETS = ["ios", "macos", "android", "windows", "linux"];
const SERVER_EXECUTABLE_NAME = process.platform === "win32" ? "native-ai-server.exe" : "native-ai-server";
const MACOS_HOST_EXECUTABLE_NAME = "NativeAIHostMac";
const MACOS_HOST_BUNDLE_NAME = "NativeAIHostMac.app";
const LINUX_HOST_EXECUTABLE_NAME = "native-ai-webapp-host";
const LINUX_HOST_APP_DIR_NAME = "NativeAIWebappHost";
const WINDOWS_HOST_EXECUTABLE_NAME = "NativeAIWebappHost.exe";
const WINDOWS_HOST_APP_DIR_NAME = "NativeAIWebappHost";
const WINDOWS_WEBVIEW2_ARCH = "x64";
const WINDOWS_WEBVIEW2_INCLUDE = path.join("build", "native", "include", "WebView2.h");
const WINDOWS_WEBVIEW2_STATIC_LIB = path.join("build", "native", WINDOWS_WEBVIEW2_ARCH, "WebView2LoaderStatic.lib");
const ZIG_CORE_ARTIFACTS = [
  {
    id: "ios-arm64-device",
    platform: "ios",
    target: "aarch64-ios",
    output: "libzig_core.a",
    args: ["--name", "zig_core", "-static", "-target", "aarch64-ios", "-lc"],
  },
  {
    id: "ios-arm64-simulator",
    platform: "ios",
    target: "aarch64-ios-simulator",
    output: "libzig_core.a",
    args: ["--name", "zig_core", "-static", "-target", "aarch64-ios-simulator", "-lc"],
  },
  {
    id: "macos-arm64",
    platform: "macos",
    target: "aarch64-macos.15.0.0",
    output: "libzig_core.a",
    args: ["--name", "zig_core", "-static", "-target", "aarch64-macos.15.0.0", "-lc"],
  },
  {
    id: "macos-x86_64",
    platform: "macos",
    target: "x86_64-macos.15.0.0",
    output: "libzig_core.a",
    args: ["--name", "zig_core", "-static", "-target", "x86_64-macos.15.0.0", "-lc"],
  },
  {
    id: "android-arm64-v8a",
    platform: "android",
    target: "aarch64-linux-android",
    output: "libzig_core.so",
    args: ["--name", "zig_core", "-dynamic", "-target", "aarch64-linux-android", "-fsoname=libzig_core.so"],
  },
  {
    id: "android-x86_64",
    platform: "android",
    target: "x86_64-linux-android",
    output: "libzig_core.so",
    args: ["--name", "zig_core", "-dynamic", "-target", "x86_64-linux-android", "-fsoname=libzig_core.so"],
  },
  {
    id: "windows-x86_64",
    platform: "windows",
    target: "x86_64-windows-gnu",
    output: "zig_core.dll",
    expectedOutputs: ["zig_core.dll", "zig_core.lib"],
    args: ["--name", "zig_core", "-dynamic", "-target", "x86_64-windows-gnu", "-lc"],
  },
  {
    id: "linux-x86_64",
    platform: "linux",
    target: "x86_64-linux-gnu",
    output: "libzig_core.so",
    args: ["--name", "zig_core", "-dynamic", "-target", "x86_64-linux-gnu", "-lc"],
  },
];

export function packageReleaseArtifacts({
  outDir = path.join(repoRoot, "artifacts"),
  buildZigCore = false,
  buildServer = false,
  buildNativeMacOS = false,
  buildNativeLinux = false,
  buildNativeWindows = false,
} = {}) {
  const resolvedOutDir = path.resolve(outDir);
  fs.mkdirSync(resolvedOutDir, { recursive: true });

  const runtimeArchive = path.join(resolvedOutDir, "runtime-web.zip");
  const examplesArchive = path.join(resolvedOutDir, "example-webapps.zip");
  const runtimeFiles = collectFiles(path.join(repoRoot, "runtime-web"), "runtime-web");
  const exampleFiles = collectFiles(path.join(repoRoot, "webapps", "examples"), "webapps/examples");

  writeStoredZip(runtimeArchive, runtimeFiles);
  writeStoredZip(examplesArchive, exampleFiles);

  const zigCoreArtifacts = buildZigCore ? buildZigCoreArtifacts({ outDir: resolvedOutDir }) : [];
  const serverArtifacts = buildServer ? buildServerArtifacts({ outDir: resolvedOutDir }) : [];
  const nativeArtifacts = [
    ...(buildNativeMacOS ? buildMacOSNativeArtifacts({ outDir: resolvedOutDir }) : []),
    ...(buildNativeLinux ? buildLinuxNativeArtifacts({ outDir: resolvedOutDir }) : []),
    ...(buildNativeWindows ? buildWindowsNativeArtifacts({ outDir: resolvedOutDir }) : []),
  ];
  const directoryArtifacts = [
    ...(buildZigCore
      ? []
      : ZIG_CORE_TARGETS.map((target) => ({
          id: `zig-core-${target}`,
          path: path.join("zig-core", target),
          description: `Target-specific Zig core library output for ${target}.`,
        }))
    ),
    ...(buildServer ? [] : [{ id: "server", path: "server", description: "Server executable output." }]),
    { id: "native-apps", path: "native-apps", description: "Target-specific native host app output." },
  ];

  for (const artifact of directoryArtifacts) {
    const artifactDir = path.join(resolvedOutDir, artifact.path);
    fs.mkdirSync(artifactDir, { recursive: true });
    fs.writeFileSync(
      path.join(artifactDir, "README.txt"),
      `${artifact.description}\nProduced by the matching CI target job or local platform build.\n`,
    );
  }

  const manifest = {
    schemaVersion: 1,
    platformVersion: PLATFORM_VERSION,
    artifacts: [
      describeFileArtifact({
        id: "runtime-web",
        archivePath: runtimeArchive,
        relativePath: "runtime-web.zip",
        source: "runtime-web/",
        fileCount: runtimeFiles.length,
      }),
      describeFileArtifact({
        id: "example-webapps",
        archivePath: examplesArchive,
        relativePath: "example-webapps.zip",
        source: "webapps/examples/",
        fileCount: exampleFiles.length,
      }),
      ...zigCoreArtifacts,
      ...serverArtifacts,
      ...nativeArtifacts,
      ...directoryArtifacts.map((artifact) => ({
        id: artifact.id,
        path: artifact.path,
        kind: "directory",
        status: "target-job-output",
      })),
    ],
  };
  const manifestPath = path.join(resolvedOutDir, "release-manifest.json");
  fs.writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);

  return {
    outDir: resolvedOutDir,
    manifestPath,
    artifacts: manifest.artifacts,
  };
}

export function buildZigCoreArtifacts({ outDir = path.join(repoRoot, "artifacts") } = {}) {
  const resolvedOutDir = path.resolve(outDir);
  const zigCoreDir = path.join(repoRoot, "zig-core");
  const headerPath = path.join(zigCoreDir, "include", "zig_core.h");
  const cacheRoot = fs.mkdtempSync(path.join(os.tmpdir(), "native-ai-zig-core-cache-"));
  const artifacts = [];

  try {
    for (const artifact of ZIG_CORE_ARTIFACTS) {
      const artifactDir = path.join(resolvedOutDir, "zig-core", artifact.platform, artifact.id);
      fs.mkdirSync(artifactDir, { recursive: true });
      const outputPath = path.join(artifactDir, artifact.output);
      const env = {
        ...process.env,
        ZIG_GLOBAL_CACHE_DIR: path.join(cacheRoot, "global"),
        ZIG_LOCAL_CACHE_DIR: path.join(cacheRoot, artifact.id),
      };
      execFileSync(
        "zig",
        ["build-lib", "src/lib.zig", ...artifact.args, `-femit-bin=${outputPath}`],
        { cwd: zigCoreDir, env, stdio: "ignore" },
      );
      fs.copyFileSync(headerPath, path.join(artifactDir, "zig_core.h"));
      const expectedOutputs = artifact.expectedOutputs ?? [artifact.output];
      const files = ["zig_core.h", ...expectedOutputs].map((fileName) =>
        describeFile(path.join(artifactDir, fileName), path.join("zig-core", artifact.platform, artifact.id, fileName)),
      );
      artifacts.push({
        id: `zig-core-${artifact.id}`,
        path: path.join("zig-core", artifact.platform, artifact.id),
        kind: "zig-core-library",
        target: artifact.target,
        files,
      });
    }
  } finally {
    fs.rmSync(cacheRoot, { recursive: true, force: true });
  }

  return artifacts;
}

export function buildServerArtifacts({ outDir = path.join(repoRoot, "artifacts") } = {}) {
  const resolvedOutDir = path.resolve(outDir);
  const serverDir = path.join(repoRoot, "server");
  const targetId = hostServerTargetId();
  const artifactDir = path.join(resolvedOutDir, "server", targetId);
  const outputPath = path.join(artifactDir, SERVER_EXECUTABLE_NAME);
  const cacheRoot = fs.mkdtempSync(path.join(os.tmpdir(), "native-ai-server-release-cache-"));
  const env = {
    ...process.env,
    ZIG_GLOBAL_CACHE_DIR: path.join(cacheRoot, "global"),
    ZIG_LOCAL_CACHE_DIR: path.join(cacheRoot, "local"),
  };

  try {
    fs.mkdirSync(artifactDir, { recursive: true });
    const moduleArgs = zigServerModuleArgs();
    const targetArgs = serverTargetArgsForHost();
    const optimizeArgs = ["-O", "ReleaseSafe"];
    if (process.platform === "darwin") {
      const objectPath = path.join(cacheRoot, "native-ai-server.o");
      execFileSync("zig", ["build-obj", ...moduleArgs, ...targetArgs, ...optimizeArgs, "-lc", `-femit-bin=${objectPath}`], {
        cwd: serverDir,
        env,
        stdio: "ignore",
      });
      execFileSync("cc", [objectPath, "-lsqlite3", "-o", outputPath], {
        env,
        stdio: "ignore",
      });
    } else {
      execFileSync(
        "zig",
        ["build-exe", ...moduleArgs, ...targetArgs, ...optimizeArgs, "-lc", "-lsqlite3", `-femit-bin=${outputPath}`],
        {
          cwd: serverDir,
          env,
          stdio: "ignore",
        },
      );
    }
    if (process.platform !== "win32") {
      fs.chmodSync(outputPath, 0o755);
    }
    return [
      {
        id: `server-${targetId}`,
        path: path.join("server", targetId),
        kind: "server-executable",
        target: targetId,
        files: [describeFile(outputPath, path.join("server", targetId, SERVER_EXECUTABLE_NAME))],
      },
    ];
  } finally {
    fs.rmSync(cacheRoot, { recursive: true, force: true });
  }
}

export function buildMacOSNativeArtifacts({ outDir = path.join(repoRoot, "artifacts") } = {}) {
  if (process.platform !== "darwin") {
    throw new Error("--build-native-macos requires a macOS host");
  }
  const resolvedOutDir = path.resolve(outDir);
  const targetId = `macos-${hostArchitectureId()}`;
  const macosDir = path.join(repoRoot, "native", "macos");
  const artifactDir = path.join(resolvedOutDir, "native-apps", "macos", targetId);
  const appBundleDir = path.join(artifactDir, MACOS_HOST_BUNDLE_NAME);
  const contentsDir = path.join(appBundleDir, "Contents");
  const macosContentsDir = path.join(contentsDir, "MacOS");
  const resourcesDir = path.join(contentsDir, "Resources");
  const frameworksDir = path.join(contentsDir, "Frameworks");
  const cacheRoot = fs.mkdtempSync(path.join(os.tmpdir(), "native-ai-macos-release-cache-"));
  const scratchPath = path.join(cacheRoot, "swiftpm");
  const moduleCachePath = path.join(cacheRoot, "module-cache");
  const env = {
    ...process.env,
    CLANG_MODULE_CACHE_PATH: moduleCachePath,
    MACOSX_DEPLOYMENT_TARGET: "13.0",
    SWIFT_MODULE_CACHE_PATH: moduleCachePath,
    SWIFTPM_MODULECACHE_OVERRIDE: moduleCachePath,
    ZIG_GLOBAL_CACHE_DIR: path.join(cacheRoot, "zig-global"),
    ZIG_LOCAL_CACHE_DIR: path.join(cacheRoot, "zig-local"),
  };

  try {
    execFileSync(
      "swift",
      [
        "build",
        "--disable-sandbox",
        "--configuration",
        "release",
        "--cache-path",
        path.join(cacheRoot, "swift-cache"),
        "--config-path",
        path.join(cacheRoot, "swift-config"),
        "--security-path",
        path.join(cacheRoot, "swift-security"),
        "--scratch-path",
        scratchPath,
        "-Xcc",
        `-fmodules-cache-path=${moduleCachePath}`,
        "-Xswiftc",
        "-module-cache-path",
        "-Xswiftc",
        moduleCachePath,
      ],
      {
        cwd: macosDir,
        env,
        stdio: "ignore",
      },
    );
    const builtExecutable = path.join(scratchPath, "release", MACOS_HOST_EXECUTABLE_NAME);
    if (!fs.existsSync(builtExecutable)) {
      throw new Error(`macOS host build did not produce ${path.relative(scratchPath, builtExecutable)}`);
    }

    fs.rmSync(appBundleDir, { recursive: true, force: true });
    fs.mkdirSync(macosContentsDir, { recursive: true });
    fs.mkdirSync(resourcesDir, { recursive: true });
    fs.mkdirSync(frameworksDir, { recursive: true });
    fs.copyFileSync(builtExecutable, path.join(macosContentsDir, MACOS_HOST_EXECUTABLE_NAME));
    fs.chmodSync(path.join(macosContentsDir, MACOS_HOST_EXECUTABLE_NAME), 0o755);
    fs.writeFileSync(path.join(contentsDir, "Info.plist"), macOSInfoPlist());

    fs.cpSync(path.join(repoRoot, "runtime-web"), path.join(resourcesDir, "runtime"), { recursive: true });
    fs.mkdirSync(path.join(resourcesDir, "webapps"), { recursive: true });
    fs.cpSync(path.join(repoRoot, "webapps", "examples"), path.join(resourcesDir, "webapps", "examples"), { recursive: true });
    fs.mkdirSync(path.join(resourcesDir, "db"), { recursive: true });
    fs.cpSync(path.join(repoRoot, "db", "sqlite"), path.join(resourcesDir, "db", "sqlite"), { recursive: true });
    buildMacOSZigCoreDylib({ outputPath: path.join(frameworksDir, "libzig_core.dylib"), env });

    return [
      {
        id: `native-macos-${targetId}`,
        path: path.join("native-apps", "macos", targetId, MACOS_HOST_BUNDLE_NAME),
        kind: "native-host-app",
        target: targetId,
        files: describeDirectoryFiles(appBundleDir, path.join("native-apps", "macos", targetId, MACOS_HOST_BUNDLE_NAME)),
      },
    ];
  } finally {
    fs.rmSync(cacheRoot, { recursive: true, force: true });
  }
}

export function buildWindowsNativeArtifacts({ outDir = path.join(repoRoot, "artifacts") } = {}) {
  if (process.platform !== "win32") {
    throw new Error("--build-native-windows requires a Windows host");
  }
  if (process.arch !== "x64") {
    throw new Error("--build-native-windows currently produces windows-x86_64 artifacts and requires an x64 host");
  }
  requireCommand("cmake", ["--version"], "--build-native-windows requires CMake on PATH");
  requireCommand("zig", ["version"], "--build-native-windows requires Zig on PATH");
  requireWindowsWebView2Sdk();

  const resolvedOutDir = path.resolve(outDir);
  const targetId = "windows-x86_64";
  const windowsDir = path.join(repoRoot, "native", "windows");
  const artifactDir = path.join(resolvedOutDir, "native-apps", "windows", targetId, WINDOWS_HOST_APP_DIR_NAME);
  const cacheRoot = fs.mkdtempSync(path.join(os.tmpdir(), "native-ai-windows-release-cache-"));
  const buildDir = path.join(cacheRoot, "cmake-build");
  const zigCoreDll = path.join(cacheRoot, "zig_core.dll");
  const env = {
    ...process.env,
    ZIG_GLOBAL_CACHE_DIR: path.join(cacheRoot, "zig-global"),
    ZIG_LOCAL_CACHE_DIR: path.join(cacheRoot, "zig-local"),
  };

  try {
    buildWindowsZigCoreDll({ outputPath: zigCoreDll, env });
    execFileSync("cmake", ["-S", windowsDir, "-B", buildDir, "-DCMAKE_BUILD_TYPE=Release"], {
      env,
      stdio: "ignore",
    });
    execFileSync("cmake", ["--build", buildDir, "--config", "Release"], {
      env,
      stdio: "ignore",
    });

    const builtExecutable = resolveWindowsHostExecutable(buildDir, "Release");
    if (!builtExecutable) {
      throw new Error("Windows host Release build did not produce NativeAIWebappHost.exe");
    }
    const builtAppDir = path.dirname(builtExecutable);
    const builtResourcesDir = path.join(builtAppDir, "resources");
    if (!fs.existsSync(path.join(builtResourcesDir, "runtime", "index.html"))) {
      throw new Error("Windows host Release build did not stage resources/runtime/index.html");
    }
    if (!fs.existsSync(path.join(builtResourcesDir, "webapps", "examples", "notes-lite", "manifest.json"))) {
      throw new Error("Windows host Release build did not stage resources/webapps/examples");
    }
    if (!fs.existsSync(path.join(builtResourcesDir, "db", "sqlite", "001_initial.sql"))) {
      throw new Error("Windows host Release build did not stage resources/db/sqlite/001_initial.sql");
    }

    fs.rmSync(artifactDir, { recursive: true, force: true });
    fs.mkdirSync(artifactDir, { recursive: true });
    fs.copyFileSync(builtExecutable, path.join(artifactDir, WINDOWS_HOST_EXECUTABLE_NAME));
    fs.cpSync(builtResourcesDir, path.join(artifactDir, "resources"), { recursive: true });
    fs.copyFileSync(zigCoreDll, path.join(artifactDir, "zig_core.dll"));

    return [
      {
        id: `native-windows-${targetId}`,
        path: path.join("native-apps", "windows", targetId, WINDOWS_HOST_APP_DIR_NAME),
        kind: "native-host-app",
        target: targetId,
        files: describeDirectoryFiles(artifactDir, path.join("native-apps", "windows", targetId, WINDOWS_HOST_APP_DIR_NAME)),
      },
    ];
  } finally {
    fs.rmSync(cacheRoot, { recursive: true, force: true });
  }
}

export function buildLinuxNativeArtifacts({ outDir = path.join(repoRoot, "artifacts") } = {}) {
  if (process.platform !== "linux") {
    throw new Error("--build-native-linux requires a Linux host");
  }
  if (process.arch !== "x64") {
    throw new Error("--build-native-linux currently produces linux-x86_64 artifacts and requires an x64 host");
  }
  requireCommand("meson", ["--version"], "--build-native-linux requires Meson on PATH");
  requireCommand("ninja", ["--version"], "--build-native-linux requires Ninja on PATH");
  requireCommand("pkg-config", ["--exists", "gtk4", "webkitgtk-6.0", "json-glib-1.0", "sqlite3", "libsoup-3.0"], "--build-native-linux requires GTK4, WebKitGTK 6.0, JSON-GLib, SQLite, and libsoup development packages");
  requireCommand("zig", ["version"], "--build-native-linux requires Zig on PATH");

  const resolvedOutDir = path.resolve(outDir);
  const targetId = "linux-x86_64";
  const linuxDir = path.join(repoRoot, "native", "linux");
  const artifactDir = path.join(resolvedOutDir, "native-apps", "linux", targetId, LINUX_HOST_APP_DIR_NAME);
  const cacheRoot = fs.mkdtempSync(path.join(os.tmpdir(), "native-ai-linux-release-cache-"));
  const buildDir = path.join(cacheRoot, "meson-build");
  const zigCoreSo = path.join(cacheRoot, "libzig_core.so");
  const env = {
    ...process.env,
    ZIG_GLOBAL_CACHE_DIR: path.join(cacheRoot, "zig-global"),
    ZIG_LOCAL_CACHE_DIR: path.join(cacheRoot, "zig-local"),
  };

  try {
    buildLinuxZigCoreSo({ outputPath: zigCoreSo, env });
    execFileSync("meson", ["setup", "--buildtype=release", buildDir, linuxDir], {
      env,
      stdio: "ignore",
    });
    execFileSync("meson", ["compile", "-C", buildDir], {
      env,
      stdio: "ignore",
    });

    const builtExecutable = path.join(buildDir, LINUX_HOST_EXECUTABLE_NAME);
    if (!fs.existsSync(builtExecutable)) {
      throw new Error(`Linux host Release build did not produce ${LINUX_HOST_EXECUTABLE_NAME}`);
    }

    fs.rmSync(artifactDir, { recursive: true, force: true });
    fs.mkdirSync(artifactDir, { recursive: true });
    fs.copyFileSync(builtExecutable, path.join(artifactDir, LINUX_HOST_EXECUTABLE_NAME));
    fs.chmodSync(path.join(artifactDir, LINUX_HOST_EXECUTABLE_NAME), 0o755);
    fs.copyFileSync(zigCoreSo, path.join(artifactDir, "libzig_core.so"));
    fs.chmodSync(path.join(artifactDir, "libzig_core.so"), 0o755);
    fs.mkdirSync(path.join(artifactDir, "resources"), { recursive: true });
    fs.cpSync(path.join(repoRoot, "runtime-web"), path.join(artifactDir, "resources", "runtime"), { recursive: true });
    fs.mkdirSync(path.join(artifactDir, "resources", "webapps"), { recursive: true });
    fs.cpSync(path.join(repoRoot, "webapps", "examples"), path.join(artifactDir, "resources", "webapps", "examples"), { recursive: true });
    fs.mkdirSync(path.join(artifactDir, "resources", "db"), { recursive: true });
    fs.cpSync(path.join(repoRoot, "db", "sqlite"), path.join(artifactDir, "resources", "db", "sqlite"), { recursive: true });

    return [
      {
        id: `native-linux-${targetId}`,
        path: path.join("native-apps", "linux", targetId, LINUX_HOST_APP_DIR_NAME),
        kind: "native-host-app",
        target: targetId,
        files: describeDirectoryFiles(artifactDir, path.join("native-apps", "linux", targetId, LINUX_HOST_APP_DIR_NAME)),
      },
    ];
  } finally {
    fs.rmSync(cacheRoot, { recursive: true, force: true });
  }
}

export function windowsWebView2SdkStatus(env = process.env) {
  const sdkDir = env.NATIVE_AI_WEBVIEW2_NUGET_DIR;
  if (!sdkDir) {
    return {
      ok: false,
      message: `NATIVE_AI_WEBVIEW2_NUGET_DIR must point to an expanded Microsoft.Web.WebView2 package containing ${WINDOWS_WEBVIEW2_INCLUDE} and ${WINDOWS_WEBVIEW2_STATIC_LIB}`,
    };
  }

  const includePath = path.join(sdkDir, WINDOWS_WEBVIEW2_INCLUDE);
  if (!fs.existsSync(includePath)) {
    return {
      ok: false,
      message: `NATIVE_AI_WEBVIEW2_NUGET_DIR is missing ${WINDOWS_WEBVIEW2_INCLUDE}: ${includePath}`,
    };
  }

  const staticLibPath = path.join(sdkDir, WINDOWS_WEBVIEW2_STATIC_LIB);
  if (!fs.existsSync(staticLibPath)) {
    return {
      ok: false,
      message: `NATIVE_AI_WEBVIEW2_NUGET_DIR is missing ${WINDOWS_WEBVIEW2_STATIC_LIB}: ${staticLibPath}`,
    };
  }

  return { ok: true, sdkDir, includePath, staticLibPath };
}

function requireWindowsWebView2Sdk() {
  const status = windowsWebView2SdkStatus();
  if (!status.ok) {
    throw new Error(`--build-native-windows requires a WebView2 SDK: ${status.message}`);
  }
}

function buildMacOSZigCoreDylib({ outputPath, env }) {
  const arch = process.arch === "arm64" ? "aarch64" : "x86_64";
  fs.mkdirSync(path.dirname(outputPath), { recursive: true });
  execFileSync(
    "zig",
    [
      "build-lib",
      "src/lib.zig",
      "--name",
      "zig_core",
      "-dynamic",
      "-target",
      `${arch}-macos.15.0.0`,
      "-lc",
      `-femit-bin=${outputPath}`,
    ],
    {
      cwd: path.join(repoRoot, "zig-core"),
      env,
      stdio: "ignore",
    },
  );
  fs.chmodSync(outputPath, 0o755);
}

function buildWindowsZigCoreDll({ outputPath, env }) {
  fs.mkdirSync(path.dirname(outputPath), { recursive: true });
  execFileSync(
    "zig",
    [
      "build-lib",
      "src/lib.zig",
      "--name",
      "zig_core",
      "-dynamic",
      "-target",
      "x86_64-windows-gnu",
      "-lc",
      `-femit-bin=${outputPath}`,
    ],
    {
      cwd: path.join(repoRoot, "zig-core"),
      env,
      stdio: "ignore",
    },
  );
}

function buildLinuxZigCoreSo({ outputPath, env }) {
  fs.mkdirSync(path.dirname(outputPath), { recursive: true });
  execFileSync(
    "zig",
    [
      "build-lib",
      "src/lib.zig",
      "--name",
      "zig_core",
      "-dynamic",
      "-target",
      "x86_64-linux-gnu",
      "-lc",
      "-fsoname=libzig_core.so",
      `-femit-bin=${outputPath}`,
    ],
    {
      cwd: path.join(repoRoot, "zig-core"),
      env,
      stdio: "ignore",
    },
  );
  fs.chmodSync(outputPath, 0o755);
}

function resolveWindowsHostExecutable(buildDir, configuration) {
  const candidates = [
    path.join(buildDir, configuration, WINDOWS_HOST_EXECUTABLE_NAME),
    path.join(buildDir, WINDOWS_HOST_EXECUTABLE_NAME),
    path.join(buildDir, "bin", configuration, WINDOWS_HOST_EXECUTABLE_NAME),
    path.join(buildDir, "bin", WINDOWS_HOST_EXECUTABLE_NAME),
  ];
  for (const candidate of candidates) {
    if (fs.existsSync(candidate)) return candidate;
  }

  const matches = walk(buildDir).filter(
    (filePath) => path.basename(filePath).toLowerCase() === WINDOWS_HOST_EXECUTABLE_NAME.toLowerCase()
      && fs.statSync(filePath).isFile(),
  );
  return matches.find((filePath) => filePath.split(path.sep).includes(configuration)) ?? matches[0] ?? null;
}

function requireCommand(command, args, message) {
  try {
    execFileSync(command, args, { stdio: "ignore" });
  } catch {
    throw new Error(message);
  }
}

function macOSInfoPlist() {
  return `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key><string>en</string>
  <key>CFBundleExecutable</key><string>${MACOS_HOST_EXECUTABLE_NAME}</string>
  <key>CFBundleIdentifier</key><string>dev.nativeai.host.macos</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleName</key><string>${MACOS_HOST_EXECUTABLE_NAME}</string>
  <key>CFBundleDisplayName</key><string>Native AI Webapp Platform</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${PLATFORM_VERSION}</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
`;
}

function zigServerModuleArgs() {
  return ["--dep", "zig_core", "-Mroot=src/main.zig", "-Mzig_core=../zig-core/src/lib.zig"];
}

function serverTargetArgsForHost() {
  if (process.platform !== "darwin") return [];
  const arch = process.arch === "arm64" ? "aarch64" : "x86_64";
  return ["-target", `${arch}-macos.15.0.0`];
}

function hostServerTargetId() {
  const platform = {
    darwin: "macos",
    linux: "linux",
    win32: "windows",
  }[process.platform];
  const arch = {
    arm64: "arm64",
    x64: "x86_64",
  }[process.arch];
  if (!platform || !arch) {
    throw new Error(`Unsupported server artifact host: ${process.platform}/${process.arch}`);
  }
  return `${platform}-${arch}`;
}

function hostArchitectureId() {
  const arch = {
    arm64: "arm64",
    x64: "x86_64",
  }[process.arch];
  if (!arch) {
    throw new Error(`Unsupported host architecture: ${process.arch}`);
  }
  return arch;
}

function describeFileArtifact({ id, archivePath, relativePath, source, fileCount }) {
  const data = fs.readFileSync(archivePath);
  return {
    id,
    path: relativePath,
    kind: "zip",
    source,
    fileCount,
    bytes: data.length,
    sha256: crypto.createHash("sha256").update(data).digest("hex"),
  };
}

function describeFile(filePath, relativePath) {
  const data = fs.readFileSync(filePath);
  return {
    path: toPosix(relativePath),
    bytes: data.length,
    sha256: crypto.createHash("sha256").update(data).digest("hex"),
  };
}

function describeDirectoryFiles(rootDir, archivePrefix) {
  return walk(rootDir)
    .filter((filePath) => fs.statSync(filePath).isFile())
    .map((filePath) => describeFile(filePath, path.join(archivePrefix, path.relative(rootDir, filePath))))
    .sort((left, right) => compareStrings(left.path, right.path));
}

function collectFiles(rootDir, archivePrefix) {
  return walk(rootDir)
    .filter((filePath) => fs.statSync(filePath).isFile())
    .map((filePath) => {
      const relativePath = toPosix(path.relative(rootDir, filePath));
      return {
        name: `${archivePrefix}/${relativePath}`,
        data: fs.readFileSync(filePath),
      };
    })
    .sort((left, right) => compareStrings(left.name, right.name));
}

function compareStrings(left, right) {
  if (left < right) return -1;
  if (left > right) return 1;
  return 0;
}

function walk(rootDir) {
  const entries = fs.readdirSync(rootDir, { withFileTypes: true });
  return entries.flatMap((entry) => {
    const absolutePath = path.join(rootDir, entry.name);
    if (entry.isDirectory()) return walk(absolutePath);
    return [absolutePath];
  });
}

function toPosix(filePath) {
  return filePath.split(path.sep).join("/");
}

function writeStoredZip(zipPath, entries) {
  const localParts = [];
  const centralParts = [];
  let offset = 0;

  for (const entry of entries) {
    const name = Buffer.from(entry.name, "utf8");
    const data = Buffer.from(entry.data);
    const crc = crc32(data);
    const localHeader = Buffer.alloc(30);
    localHeader.writeUInt32LE(0x04034b50, 0);
    localHeader.writeUInt16LE(10, 4);
    localHeader.writeUInt16LE(0, 6);
    localHeader.writeUInt16LE(0, 8);
    localHeader.writeUInt16LE(FIXED_DOS_TIME, 10);
    localHeader.writeUInt16LE(FIXED_DOS_DATE, 12);
    localHeader.writeUInt32LE(crc, 14);
    localHeader.writeUInt32LE(data.length, 18);
    localHeader.writeUInt32LE(data.length, 22);
    localHeader.writeUInt16LE(name.length, 26);
    localHeader.writeUInt16LE(0, 28);
    localParts.push(localHeader, name, data);

    const centralHeader = Buffer.alloc(46);
    centralHeader.writeUInt32LE(0x02014b50, 0);
    centralHeader.writeUInt16LE(20, 4);
    centralHeader.writeUInt16LE(10, 6);
    centralHeader.writeUInt16LE(0, 8);
    centralHeader.writeUInt16LE(0, 10);
    centralHeader.writeUInt16LE(FIXED_DOS_TIME, 12);
    centralHeader.writeUInt16LE(FIXED_DOS_DATE, 14);
    centralHeader.writeUInt32LE(crc, 16);
    centralHeader.writeUInt32LE(data.length, 20);
    centralHeader.writeUInt32LE(data.length, 24);
    centralHeader.writeUInt16LE(name.length, 28);
    centralHeader.writeUInt16LE(0, 30);
    centralHeader.writeUInt16LE(0, 32);
    centralHeader.writeUInt16LE(0, 34);
    centralHeader.writeUInt16LE(0, 36);
    centralHeader.writeUInt32LE(0, 38);
    centralHeader.writeUInt32LE(offset, 42);
    centralParts.push(centralHeader, name);

    offset += localHeader.length + name.length + data.length;
  }

  const centralDirectory = Buffer.concat(centralParts);
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0);
  end.writeUInt16LE(0, 4);
  end.writeUInt16LE(0, 6);
  end.writeUInt16LE(entries.length, 8);
  end.writeUInt16LE(entries.length, 10);
  end.writeUInt32LE(centralDirectory.length, 12);
  end.writeUInt32LE(offset, 16);
  end.writeUInt16LE(0, 20);

  fs.writeFileSync(zipPath, Buffer.concat([...localParts, centralDirectory, end]));
}

export function listZipEntries(zipPath) {
  const data = fs.readFileSync(zipPath);
  const entries = [];
  for (let offset = 0; offset < data.length - 4; ) {
    const signature = data.readUInt32LE(offset);
    if (signature !== 0x04034b50) break;
    const compressedSize = data.readUInt32LE(offset + 18);
    const nameLength = data.readUInt16LE(offset + 26);
    const extraLength = data.readUInt16LE(offset + 28);
    const nameStart = offset + 30;
    const name = data.subarray(nameStart, nameStart + nameLength).toString("utf8");
    entries.push(name);
    offset = nameStart + nameLength + extraLength + compressedSize;
  }
  return entries;
}

const CRC32_TABLE = makeCrc32Table();

function makeCrc32Table() {
  return Array.from({ length: 256 }, (_, index) => {
    let value = index;
    for (let bit = 0; bit < 8; bit += 1) {
      value = value & 1 ? 0xedb88320 ^ (value >>> 1) : value >>> 1;
    }
    return value >>> 0;
  });
}

function crc32(data) {
  let crc = 0xffffffff;
  for (const byte of data) {
    crc = CRC32_TABLE[(crc ^ byte) & 0xff] ^ (crc >>> 8);
  }
  return (crc ^ 0xffffffff) >>> 0;
}

function parseCliArgs(argv) {
  const options = {};
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--out") {
      options.outDir = path.resolve(argv[(index += 1)]);
    } else if (arg === "--build-zig-core") {
      options.buildZigCore = true;
    } else if (arg === "--build-server") {
      options.buildServer = true;
    } else if (arg === "--build-native-macos") {
      options.buildNativeMacOS = true;
    } else if (arg === "--build-native-linux") {
      options.buildNativeLinux = true;
    } else if (arg === "--build-native-windows") {
      options.buildNativeWindows = true;
    } else {
      throw new Error(`Unknown argument: ${arg}`);
    }
  }
  return options;
}

const currentFile = fileURLToPath(import.meta.url);
if (process.argv[1] && path.resolve(process.argv[1]) === currentFile) {
  try {
    const result = packageReleaseArtifacts(parseCliArgs(process.argv.slice(2)));
    console.log(JSON.stringify(result, null, 2));
  } catch (error) {
    console.error(error.stack ?? error.message);
    process.exitCode = 1;
  }
}
