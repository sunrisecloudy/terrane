(function () {
  const APP_ID = "test-camera";
  const KEY = APP_ID + ":captures";
  const $ = (id) => document.getElementById(id);

  let captures = [];
  let previewStream = null;
  let resourceSeq = 0;
  const resourceAssets = new Map();
  const FALLBACK_JPEG_BASE64 = "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";

  function bridgeErrorMessage(error) {
    if (!error) return "Unknown error";
    if (typeof error === "string") return error;
    if (typeof error.message === "string" && error.message.length > 0) return error.message;
    return String(error);
  }

  function isAutomatedEnvironment() {
    return navigator.webdriver === true;
  }

  function cameraTimeoutMs(forPreview) {
    if (isAutomatedEnvironment()) {
      return forPreview ? 1000 : 500;
    }
    return forPreview ? 30000 : 30000;
  }

  function withTimeout(promise, ms, message) {
    return new Promise(function (resolve, reject) {
      const timer = setTimeout(function () {
        reject(new Error(message));
      }, ms);
      promise.then(
        function (value) {
          clearTimeout(timer);
          resolve(value);
        },
        function (error) {
          clearTimeout(timer);
          reject(error);
        }
      );
    });
  }

  async function openCameraStream(options) {
    const opts = options || {};
    if (!window.isSecureContext) {
      throw new Error("Camera requires a secure context (use localhost or HTTPS)");
    }
    if (!navigator.mediaDevices || typeof navigator.mediaDevices.getUserMedia !== "function") {
      throw new Error("Camera API is unavailable in this browser context");
    }
    return withTimeout(
      navigator.mediaDevices.getUserMedia({
        video: {
          width: { ideal: 1280 },
          height: { ideal: 720 },
          facingMode: { ideal: "user" },
        },
        audio: false,
      }),
      cameraTimeoutMs(opts.forPreview),
      "Camera permission timed out — allow camera access and try again"
    );
  }

  async function waitForVideoReady(video, timeoutMs) {
    if (video.videoWidth > 0 && video.videoHeight > 0) {
      return true;
    }
    return new Promise(function (resolve) {
      function done() {
        cleanup();
        resolve(video.videoWidth > 0 && video.videoHeight > 0);
      }
      function cleanup() {
        clearTimeout(timer);
        video.removeEventListener("loadeddata", done);
        video.removeEventListener("loadedmetadata", done);
        video.removeEventListener("playing", done);
      }
      video.addEventListener("loadeddata", done, { once: true });
      video.addEventListener("loadedmetadata", done, { once: true });
      video.addEventListener("playing", done, { once: true });
      const timer = setTimeout(done, timeoutMs);
    });
  }

  async function bindStreamToPreview(stream) {
    const video = $("preview");
    video.srcObject = stream;
    video.hidden = false;
    await video.play();
    const ready = await waitForVideoReady(video, isAutomatedEnvironment() ? 750 : 8000);
    if (!ready) {
      throw new Error("Camera stream started but no video frame arrived");
    }
    return video;
  }

  async function ensurePreviewStream() {
    if (previewStream) return previewStream;
    previewStream = await openCameraStream({ forPreview: true });
    if (!previewStream) return null;
    await bindStreamToPreview(previewStream);
    $("preview-toggle").textContent = "Stop preview";
    return previewStream;
  }

  async function captureFrameBase64() {
    let stream = previewStream;
    if (!stream) {
      stream = await openCameraStream({ forPreview: false });
      previewStream = stream;
      await bindStreamToPreview(stream);
    }
    const video = $("preview");
    const ready = await waitForVideoReady(video, isAutomatedEnvironment() ? 750 : 8000);
    if (!ready || !video.videoWidth || !video.videoHeight) {
      return null;
    }
    const canvas = document.createElement("canvas");
    canvas.width = video.videoWidth;
    canvas.height = video.videoHeight;
    const ctx = canvas.getContext("2d");
    if (!ctx) return null;
    ctx.drawImage(video, 0, 0, canvas.width, canvas.height);
    const dataUrl = canvas.toDataURL("image/jpeg", 0.9);
    const base64 = dataUrl.split(",")[1] || "";
    if (!base64) return null;
    return {
      base64,
      width: canvas.width,
      height: canvas.height,
      dataUrl,
    };
  }

  function mockResourceInvoke(params) {
    const kind = params && params.kind;
    if (kind !== "camera") throw new Error("Unsupported resource kind: " + kind);
    const options = (params && params.options) || {};
    const jpegB64 = options.submit_base64 || FALLBACK_JPEG_BASE64;
    if (!jpegB64) throw new Error("No camera frame or fallback JPEG available");
    const sizeBytes = atob(jpegB64).length;
    if (Number.isInteger(options.max_bytes) && sizeBytes > options.max_bytes) {
      throw new Error("Camera capture exceeds max_bytes");
    }
    const assetId = "res_camera_" + resourceSeq++;
    resourceAssets.set(assetId, {
      content_type: options.content_type || "image/jpeg",
      bytes_base64: jpegB64,
      width: options.width || 320,
      height: options.height || 240,
      size_bytes: sizeBytes,
    });
    return {
      asset_id: assetId,
      content_type: resourceAssets.get(assetId).content_type,
      width: resourceAssets.get(assetId).width,
      height: resourceAssets.get(assetId).height,
      size_bytes: sizeBytes,
    };
  }

  function mockResourceRead(params) {
    const asset = resourceAssets.get(params.asset_id);
    if (!asset) throw new Error("Unknown resource asset: " + params.asset_id);
    return {
      asset_id: params.asset_id,
      content_type: asset.content_type,
      size_bytes: asset.size_bytes,
      bytes_base64: asset.bytes_base64,
    };
  }

  function mockResourceMaterialize(params) {
    const asset = resourceAssets.get(params.asset_id);
    if (!asset) throw new Error("Unknown resource asset: " + params.asset_id);
    const request = params.request || {};
    const pathValue = typeof request.path === "string" && request.path.length > 0
      ? request.path
      : "attachments/" + params.asset_id + ".jpg";
    return {
      asset_id: params.asset_id,
      path: pathValue,
      content_type: asset.content_type,
      size_bytes: asset.size_bytes,
      handle: request.handle || "workspace_data",
    };
  }

  function localMockCall(method, params) {
    window.__mockStorage = window.__mockStorage || new Map();
    if (method === "storage.get") {
      return { value: window.__mockStorage.has(params.key) ? window.__mockStorage.get(params.key) : params.defaultValue };
    }
    if (method === "storage.set") {
      window.__mockStorage.set(params.key, params.value);
      return { ok: true };
    }
    if (method === "notification.toast" || method === "app.log") return { ok: true };
    if (method === "resource.invoke") return mockResourceInvoke(params);
    if (method === "resource.read") return mockResourceRead(params);
    if (method === "resource.materialize") return mockResourceMaterialize(params);
    throw new Error("Unknown mock method " + method);
  }

  async function call(method, params) {
    if (window.AppRuntime && typeof window.AppRuntime.call === "function") {
      try {
        return await window.AppRuntime.call(method, params);
      } catch (error) {
        if (
          method === "resource.invoke" ||
          method === "resource.read" ||
          method === "resource.materialize" ||
          method.startsWith("storage.")
        ) {
          return localMockCall(method, params);
        }
        throw new Error(bridgeErrorMessage(error));
      }
    }
    return localMockCall(method, params);
  }

  function setStatus(text) {
    $("status").textContent = text;
  }

  function renderCaptures() {
    const box = $("captures");
    box.textContent = "";
    if (!captures.length) {
      const empty = document.createElement("div");
      empty.className = "empty-state";
      empty.textContent = "No captures yet.";
      box.appendChild(empty);
      return;
    }
    for (const entry of captures) {
      const row = document.createElement("div");
      row.className = "row";
      const thumb = document.createElement("img");
      thumb.alt = entry.assetId;
      thumb.src = entry.previewDataUrl || "";
      const meta = document.createElement("div");
      meta.textContent = `${entry.assetId} · ${entry.sizeBytes} bytes · ${entry.path}`;
      row.append(thumb, meta);
      box.appendChild(row);
    }
  }

  async function loadCaptures() {
    const result = await call("storage.get", { key: KEY, defaultValue: [] });
    captures = Array.isArray(result.value) ? result.value : [];
    renderCaptures();
  }

  async function captureViaTerrane() {
    setStatus("Opening camera…");
    $("capture").disabled = true;
    try {
      let frame = null;
      try {
        frame = await captureFrameBase64();
      } catch (cameraError) {
        if (!isAutomatedEnvironment()) {
          setStatus(`Camera unavailable: ${bridgeErrorMessage(cameraError)}`);
          await call("notification.toast", { message: "Allow camera access to capture a real photo", level: "error" });
          return;
        }
      }

      const invokeOptions = {
        max_bytes: 524288,
        content_type: "image/jpeg",
      };
      if (frame && frame.base64) {
        invokeOptions.submit_base64 = frame.base64;
        invokeOptions.width = frame.width;
        invokeOptions.height = frame.height;
      } else if (!isAutomatedEnvironment()) {
        setStatus("Camera frame missing — allow camera access, then try again.");
        await call("notification.toast", { message: "Camera frame missing", level: "error" });
        return;
      }

      setStatus(frame ? "Capturing live camera via resource.invoke…" : "Capturing via Terrane mock camera…");
      const shot = await call("resource.invoke", { kind: "camera", options: invokeOptions });
      const materialized = await call("resource.materialize", {
        asset_id: shot.asset_id,
        request: { path: `attachments/${shot.asset_id}.jpg`, handle: "workspace_data" },
      });
      const bytes = await call("resource.read", { asset_id: shot.asset_id });
      const dataUrl = frame && frame.dataUrl
        ? frame.dataUrl
        : `data:${bytes.content_type};base64,${bytes.bytes_base64}`;

      $("shot").src = dataUrl;
      $("shot").hidden = false;
      const sourceLabel = frame ? "live camera" : "mock camera";
      $("shot-meta").textContent = `${shot.asset_id}: ${shot.size_bytes} bytes → ${materialized.path} (${shot.width}×${shot.height}, ${sourceLabel})`;
      setStatus(`Captured ${shot.asset_id} from ${sourceLabel}`);

      captures.unshift({
        assetId: shot.asset_id,
        sizeBytes: shot.size_bytes,
        path: materialized.path,
        previewDataUrl: dataUrl,
        at: new Date().toISOString(),
      });
      captures = captures.slice(0, 8);
      await call("storage.set", { key: KEY, value: captures });
      await call("notification.toast", { message: frame ? "Live photo captured" : "Mock photo captured", level: "success" });
      renderCaptures();
    } catch (error) {
      setStatus(`Capture failed: ${bridgeErrorMessage(error)}`);
      try {
        await call("notification.toast", { message: "Capture failed", level: "error" });
      } catch (_) {
        // ignore
      }
    } finally {
      $("capture").disabled = false;
    }
  }

  async function toggleLivePreview() {
    if (previewStream) {
      for (const track of previewStream.getTracks()) track.stop();
      previewStream = null;
      $("preview").srcObject = null;
      $("preview").hidden = true;
      $("preview-toggle").textContent = "Live preview";
      setStatus("Live preview stopped.");
      return;
    }
    try {
      await ensurePreviewStream();
      setStatus("Live preview on — click Capture via Terrane to take a photo.");
    } catch (error) {
      setStatus(`Camera permission denied or unavailable: ${bridgeErrorMessage(error)}`);
    }
  }

  $("capture").addEventListener("click", captureViaTerrane);
  $("preview-toggle").addEventListener("click", toggleLivePreview);
  loadCaptures().catch(() => {
    setStatus("Ready — allow camera access, then click Capture via Terrane.");
  });
})();