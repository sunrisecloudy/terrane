import { useEffect, useMemo, useState } from "react";

import {
  acknowledgeHealthAnalysis,
  autoUploadHealthImage,
  fileToBase64,
  hasHealthAutoUpload,
  hasHealthRemoteAnalysis,
  hasPhotosPicker,
  healthAnalysisStatus,
  invokeJson,
  pickPhoto,
  submitHealthAnalysis,
} from "../terrane";
import type { BlobItem, MealEntry, MutationResult, Settings } from "../types";

const imageTypes = ["image/jpeg", "image/png", "image/webp"];

export function AddMealRoute({
  settings,
  onSettings,
  onEstimated,
}: {
  settings: Settings;
  onSettings: (settings: Settings) => void;
  onEstimated: (entry: MealEntry, history: MealEntry[]) => void;
}) {
  const [file, setFile] = useState<File | null>(null);
  const [nativeBlob, setNativeBlob] = useState<BlobItem | null>(null);
  const [note, setNote] = useState("");
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState("");
  const [syncStatus, setSyncStatus] = useState("");
  const [analysisJobID, setAnalysisJobID] = useState("");
  const [error, setError] = useState("");
  const [dragging, setDragging] = useState(false);
  const preview = useMemo(
    () =>
      nativeBlob
        ? window.terrane?.blobUrl?.(nativeBlob.name) || ""
        : file
        ? URL.createObjectURL(file)
        : "",
    [file, nativeBlob],
  );

  useEffect(() => {
    return () => {
      if (file && preview) URL.revokeObjectURL(preview);
      if (nativeBlob) {
        invokeJson("discard_import", nativeBlob.name).catch(() => {});
      }
    };
  }, [file, nativeBlob, preview]);

  useEffect(() => {
    if (!hasHealthRemoteAnalysis()) return;
    window.terrane?.pendingHealthAnalyses?.()
      .then((pending) => {
        if (pending.jobIds.length > 0) {
          setBusy(true);
          setStatus("Resuming nutrition from your connected Mac…");
          setAnalysisJobID(pending.jobIds[pending.jobIds.length - 1]);
        }
      })
      .catch(() => {});
  }, []);

  useEffect(() => {
    if (!analysisJobID) return;
    let cancelled = false;
    let timer = 0;
    const poll = async () => {
      try {
        const update = await healthAnalysisStatus(analysisJobID);
        if (cancelled) return;
        if (
          update.status === "completed" &&
          update.imageBase64 &&
          update.mime &&
          update.resultJson
        ) {
          setStatus("Saving nutrition on this iPhone…");
          const result = await invokeJson<MutationResult>(
            "import_analysis",
            update.jobId,
            update.imageBase64,
            update.mime,
            update.resultJson,
          );
          if (!result.ok || !result.estimate) {
            throw new Error(
              result.error || "Could not save the nutrition result.",
            );
          }
          await acknowledgeHealthAnalysis(update.jobId);
          if (cancelled) return;
          setAnalysisJobID("");
          setFile(null);
          setBusy(false);
          setSyncStatus("Encrypted nutrition received from your Mac");
          onEstimated(result.estimate, result.history || []);
          return;
        }
        if (update.status === "failed" || update.status === "cancelled") {
          throw new Error(
            update.failureCode
              ? `Mac analysis failed: ${
                update.failureCode.replaceAll("_", " ")
              }`
              : "Mac analysis did not complete.",
          );
        }
        setStatus(
          update.status === "claimed" ||
            update.status === "leased" ||
            update.status === "processing"
            ? "Your Mac is analyzing this meal…"
            : "Waiting for your connected Mac…",
        );
        timer = window.setTimeout(poll, 1_000);
      } catch (caught) {
        if (cancelled) return;
        setAnalysisJobID("");
        setBusy(false);
        setError(
          caught instanceof Error
            ? caught.message
            : "Could not receive Mac analysis.",
        );
      }
    };
    void poll();
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [analysisJobID, onEstimated]);

  function selectFile(next: File | null) {
    if (!next || !imageTypes.includes(next.type)) {
      setError("Choose a JPEG, PNG, or WebP food photo.");
      return;
    }
    if (nativeBlob) {
      invokeJson("discard_import", nativeBlob.name).catch(() => {});
    }
    setNativeBlob(null);
    setFile(next);
    setError("");
    if (hasHealthRemoteAnalysis()) {
      setBusy(true);
      setStatus("Encrypting photo for your Mac…");
      setSyncStatus("");
      submitHealthAnalysis(next, note)
        .then((result) => {
          setAnalysisJobID(result.jobId);
          setStatus("Waiting for your connected Mac…");
          setSyncStatus("Encrypted photo uploaded");
        })
        .catch((caught) => {
          setBusy(false);
          setError(
            caught instanceof Error
              ? `Connected Mac analysis unavailable: ${caught.message}`
              : "Connected Mac analysis unavailable",
          );
        });
    } else if (hasHealthAutoUpload()) {
      setSyncStatus("Encrypting and syncing to desktop…");
      autoUploadHealthImage(next)
        .then((result) => {
          setSyncStatus(result?.ok ? "Encrypted photo uploaded" : "");
        })
        .catch((caught) => {
          setSyncStatus(
            caught instanceof Error
              ? `Encrypted sync unavailable: ${caught.message}`
              : "Encrypted sync unavailable",
          );
        });
    }
  }

  async function openPhotos() {
    setError("");
    try {
      const result = await pickPhoto();
      if (result.cancelled) return;
      const item = result.items[0];
      if (!item || item.kind !== "blob" || item.mime !== "image/jpeg") {
        throw new Error("Photos returned an invalid image.");
      }
      if (nativeBlob) {
        await invokeJson("discard_import", nativeBlob.name).catch(() => {});
      }
      setFile(null);
      setNativeBlob(item);
    } catch (caught) {
      setError(
        caught instanceof Error ? caught.message : "Could not open Photos.",
      );
    }
  }

  async function estimate() {
    if ((!file && !nativeBlob) || busy) return;
    setBusy(true);
    setError("");
    setStatus(file ? "Preparing image…" : "Identifying dishes…");
    let waitingForRemote = false;
    try {
      if (file && hasHealthRemoteAnalysis()) {
        const submitted = await submitHealthAnalysis(file, note);
        waitingForRemote = true;
        setAnalysisJobID(submitted.jobId);
        setStatus("Waiting for your connected Mac…");
        return;
      }
      let result: MutationResult;
      if (nativeBlob) {
        result = await invokeJson(
          "estimate_blob",
          nativeBlob.name,
          note,
          settings.provider,
          settings.model,
        );
      } else {
        const selected = file as File;
        const base64 = await fileToBase64(selected);
        setStatus("Identifying dishes and estimating nutrition…");
        result = await invokeJson(
          "estimate",
          base64,
          selected.type,
          note,
          settings.provider,
          settings.model,
        );
      }
      if (!result.ok || !result.estimate) {
        throw new Error(result.error || "Could not estimate this meal.");
      }
      setNativeBlob(null);
      onEstimated(result.estimate, result.history || []);
    } catch (caught) {
      if (nativeBlob) {
        await invokeJson("discard_import", nativeBlob.name).catch(() => {});
        setNativeBlob(null);
      }
      setError(
        caught instanceof Error
          ? caught.message
          : "Could not estimate this meal.",
      );
    } finally {
      if (!waitingForRemote) {
        setBusy(false);
        if (!hasHealthRemoteAnalysis()) setStatus("");
      }
    }
  }

  async function updateSettings(next: Settings) {
    onSettings(next);
    try {
      await invokeJson("configure", next.provider, next.model);
    } catch (caught) {
      setError(
        caught instanceof Error ? caught.message : "Could not save settings.",
      );
    }
  }

  return (
    <div className="route-stack">
      <div className="route-heading">
        <div>
          <h2>Add meal</h2>
          <p className="subtitle">
            Photograph a meal and review a dish-by-dish estimate.
          </p>
        </div>
      </div>
      <section className="panel">
        <div className="panel-body add-meal-grid">
          <div>
            <h3 className="panel-title">Add a food photo</h3>
            <p className="panel-help">
              Use a clear, well-lit photo that shows the whole serving.
            </p>
            <div
              className={`drop${preview ? " has-photo" : ""}${
                dragging ? " dragging" : ""
              }`}
              onDragEnter={(event) => {
                event.preventDefault();
                setDragging(true);
              }}
              onDragOver={(event) => event.preventDefault()}
              onDragLeave={(event) => {
                event.preventDefault();
                setDragging(false);
              }}
              onDrop={(event) => {
                event.preventDefault();
                setDragging(false);
                selectFile(event.dataTransfer.files[0] || null);
              }}
            >
              {preview
                ? <img className="preview" src={preview} alt="Selected food" />
                : null}
              <div className="drop-content">
                <div className="upload-icon">◒</div>
                <div className="upload-title">
                  {preview ? "Food photo selected" : "Drop a photo here"}
                </div>
                <div className="upload-detail">
                  {nativeBlob
                    ? `${nativeBlob.width} × ${nativeBlob.height} normalized JPEG`
                    : "JPEG, PNG, or WebP"}
                </div>
                <div className="photo-actions">
                  {hasPhotosPicker()
                    ? (
                      <button
                        className="photos-button"
                        type="button"
                        onClick={openPhotos}
                      >
                        Choose from Photos
                      </button>
                    )
                    : null}
                  <label className="choose">
                    Choose file
                    <input
                      className="file-input"
                      type="file"
                      accept="image/jpeg,image/png,image/webp"
                      onChange={(event) =>
                        selectFile(event.target.files?.[0] || null)}
                    />
                  </label>
                </div>
              </div>
            </div>
          </div>
          <div className="meal-options">
            <div className="model-controls">
              <label className="model-control">
                <span>Vision provider</span>
                <select
                  value={settings.provider}
                  disabled={hasHealthRemoteAnalysis()}
                  onChange={(event) =>
                    updateSettings({
                      ...settings,
                      provider: event.target.value === "codex"
                        ? "codex"
                        : event.target.value === "claude"
                        ? "claude"
                        : "opencode",
                    })}
                >
                  <option value="opencode">OpenCode</option>
                  <option value="codex">Codex</option>
                  <option value="claude">Claude</option>
                </select>
              </label>
              <label className="model-control">
                <span>Vision model</span>
                <input
                  value={settings.model}
                  disabled={hasHealthRemoteAnalysis() ||
                    settings.provider !== "opencode"}
                  onChange={(event) =>
                    onSettings({ ...settings, model: event.target.value })}
                  onBlur={() => updateSettings(settings)}
                  list="vision-models"
                />
                <datalist id="vision-models">
                  <option value="opencode-go/kimi-k2.6">
                    Kimi K2.6 — OpenCode Go
                  </option>
                  <option value="openrouter/google/gemini-2.5-flash">
                    Gemini 2.5 Flash
                  </option>
                  <option value="openrouter/qwen/qwen3-vl-32b-instruct">
                    Qwen3 VL 32B
                  </option>
                </datalist>
              </label>
              <p className="model-help">
                {hasHealthRemoteAnalysis()
                  ? "Your connected Mac analyzes securely with OpenCode and Kimi K2.6."
                  : settings.provider === "opencode"
                  ? "OpenCode uses Kimi K2.6 by default."
                  : settings.provider === "claude"
                  ? "Claude uses your signed-in Claude CLI configuration."
                  : "Codex uses your signed-in Codex CLI configuration."}
              </p>
            </div>
            <label className="note-label">
              Meal details <span>(optional)</span>
            </label>
            <textarea
              maxLength={500}
              value={note}
              onChange={(event) => setNote(event.target.value)}
              placeholder="e.g. grilled chicken, dressing on the side"
            />
            <button
              className="estimate-button"
              type="button"
              disabled={busy || (!file && !nativeBlob)}
              onClick={estimate}
            >
              {busy
                ? hasHealthRemoteAnalysis()
                  ? "Analyzing on your Mac…"
                  : "Estimating nutrition…"
                : "Estimate nutrition"}
            </button>
            {busy
              ? (
                <div className="status visible" role="status">
                  <span className="spinner" />
                  <span>{status}</span>
                </div>
              )
              : null}
            {error
              ? <div className="error visible" role="alert">{error}</div>
              : null}
            {syncStatus
              ? (
                <div className="status visible" role="status">
                  <span>{syncStatus}</span>
                </div>
              )
              : null}
            <p className="disclaimer">
              Estimates are approximate and can vary with portion size and
              ingredients. Review against labels when accuracy matters. This is
              not medical advice.
            </p>
          </div>
        </div>
      </section>
    </div>
  );
}
