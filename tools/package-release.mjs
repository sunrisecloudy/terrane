#!/usr/bin/env node
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

export const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

const FIXED_DOS_TIME = 0;
const FIXED_DOS_DATE = 33;
const PLATFORM_VERSION = "0.1.0";
const ZIG_CORE_TARGETS = ["ios", "macos", "android", "windows", "linux"];

export function packageReleaseArtifacts({ outDir = path.join(repoRoot, "artifacts") } = {}) {
  const resolvedOutDir = path.resolve(outDir);
  fs.mkdirSync(resolvedOutDir, { recursive: true });

  const runtimeArchive = path.join(resolvedOutDir, "runtime-web.zip");
  const examplesArchive = path.join(resolvedOutDir, "example-webapps.zip");
  const runtimeFiles = collectFiles(path.join(repoRoot, "runtime-web"), "runtime-web");
  const exampleFiles = collectFiles(path.join(repoRoot, "webapps", "examples"), "webapps/examples");

  writeStoredZip(runtimeArchive, runtimeFiles);
  writeStoredZip(examplesArchive, exampleFiles);

  const directoryArtifacts = [
    ...ZIG_CORE_TARGETS.map((target) => ({
      id: `zig-core-${target}`,
      path: path.join("zig-core", target),
      description: `Target-specific Zig core library output for ${target}.`,
    })),
    { id: "server", path: "server", description: "Server executable output." },
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
