import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { PlatformError } from "./errors.js";

const FIXTURE_PATH = path.join(path.dirname(fileURLToPath(import.meta.url)), "../fixtures/mock-camera.jpg");

/** Visible JPEG used by the reference-host mock camera (not the 22-byte forge replay stub). */
export const MOCK_JPEG_BYTES = fs.readFileSync(FIXTURE_PATH);

export class ResourceMockStore {
  constructor() {
    this.seq = 0;
    /** @type {Map<string, { bytes: Buffer, contentType: string, width: number, height: number }>} */
    this.assets = new Map();
  }

  invoke(kind, options = {}) {
    if (kind !== "camera") {
      throw new PlatformError("invalid_request", `Unsupported resource kind: ${kind}`, { kind });
    }

    let bytes = MOCK_JPEG_BYTES;
    let width = 320;
    let height = 240;
    if (typeof options.submit_base64 === "string" && options.submit_base64.length > 0) {
      bytes = Buffer.from(options.submit_base64, "base64");
      if (Number.isInteger(options.width)) width = options.width;
      if (Number.isInteger(options.height)) height = options.height;
    }

    if (Number.isInteger(options.max_bytes) && bytes.length > options.max_bytes) {
      throw new PlatformError("resource_budget_exceeded", "Camera capture exceeds max_bytes", {
        size_bytes: bytes.length,
        max_bytes: options.max_bytes,
      });
    }

    const assetId = `res_camera_${this.seq}`;
    this.seq += 1;
    const contentType = typeof options.content_type === "string" ? options.content_type : "image/jpeg";
    const blob = { bytes, contentType, width, height };
    this.assets.set(assetId, blob);
    return {
      asset_id: assetId,
      content_type: contentType,
      width: blob.width,
      height: blob.height,
      size_bytes: bytes.length,
    };
  }

  read(assetId) {
    const asset = this.assets.get(assetId);
    if (!asset) {
      throw new PlatformError("invalid_request", `Unknown resource asset: ${assetId}`, { asset_id: assetId });
    }
    return {
      asset_id: assetId,
      content_type: asset.contentType,
      size_bytes: asset.bytes.length,
      bytes_base64: asset.bytes.toString("base64"),
    };
  }

  materialize(assetId, request = {}) {
    const asset = this.assets.get(assetId);
    if (!asset) {
      throw new PlatformError("invalid_request", `Unknown resource asset: ${assetId}`, { asset_id: assetId });
    }
    const pathValue = typeof request.path === "string" && request.path.length > 0
      ? request.path
      : `attachments/${assetId}.jpg`;
    return {
      asset_id: assetId,
      path: pathValue,
      content_type: asset.contentType,
      size_bytes: asset.bytes.length,
      handle: typeof request.handle === "string" ? request.handle : "workspace_data",
    };
  }
}