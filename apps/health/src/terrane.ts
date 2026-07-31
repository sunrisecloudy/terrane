import type { PickResult } from "./types";

type SidebarItem = {
  id: string;
  title: string;
  systemImage?: string;
};

type SidebarSection = {
  title: string;
  items: SidebarItem[];
  selectedItemId?: string;
};

type TerraneApi = {
  invoke?: (verb: string, ...args: string[]) => Promise<string>;
  blobUrl?: (name: string) => string;
  pick?: (options: {
    source: "photos";
    types: ["image"];
    multiple: false;
  }) => Promise<PickResult>;
  uploadHealthImage?: (
    base64: string,
    mime: string,
  ) => Promise<{ ok: boolean; attachmentId: string; clientId: string }>;
  analyzeHealthImage?: (
    base64: string,
    mime: string,
    note: string,
  ) => Promise<{
    ok: boolean;
    attachmentId: string;
    jobId: string;
    status: string;
  }>;
  healthAnalysisStatus?: (jobId: string) => Promise<{
    ok: boolean;
    jobId: string;
    status: string;
    failureCode?: string;
    imageBase64?: string;
    mime?: string;
    resultJson?: string;
  }>;
  acknowledgeHealthAnalysis?: (jobId: string) => Promise<{ ok: boolean }>;
  pendingHealthAnalyses?: () => Promise<{ ok: boolean; jobIds: string[] }>;
  setSidebarSection?: (section: SidebarSection) => void;
  onSidebarItemSelect?: (callback: (id: string) => void) => void;
  onSidebarCreate?: (callback: () => void) => void;
};

declare global {
  interface Window {
    terrane?: TerraneApi;
  }
}

export async function invokeJson<T>(
  verb: string,
  ...args: string[]
): Promise<T> {
  if (!window.terrane || typeof window.terrane.invoke !== "function") {
    throw new Error("Terrane bridge unavailable");
  }
  return JSON.parse(await window.terrane.invoke(verb, ...args)) as T;
}

export function blobUrl(name: string): string {
  return window.terrane && typeof window.terrane.blobUrl === "function"
    ? window.terrane.blobUrl(name)
    : "";
}

export function hasPhotosPicker(): boolean {
  return Boolean(window.terrane && typeof window.terrane.pick === "function");
}

export function hasHealthAutoUpload(): boolean {
  return Boolean(
    window.terrane && typeof window.terrane.uploadHealthImage === "function",
  );
}

export function hasHealthRemoteAnalysis(): boolean {
  return Boolean(
    window.terrane &&
      typeof window.terrane.analyzeHealthImage === "function" &&
      typeof window.terrane.healthAnalysisStatus === "function" &&
      typeof window.terrane.acknowledgeHealthAnalysis === "function",
  );
}

export async function autoUploadHealthImage(
  file: File,
): Promise<{ ok: boolean; attachmentId: string; clientId: string } | null> {
  if (
    !window.terrane || typeof window.terrane.uploadHealthImage !== "function"
  ) {
    return null;
  }
  return window.terrane.uploadHealthImage(await fileToBase64(file), file.type);
}

export async function submitHealthAnalysis(file: File, note: string) {
  if (!window.terrane?.analyzeHealthImage) {
    throw new Error("Connected Mac analysis is unavailable.");
  }
  return window.terrane.analyzeHealthImage(
    await fileToBase64(file),
    file.type,
    note,
  );
}

export async function healthAnalysisStatus(jobId: string) {
  if (!window.terrane?.healthAnalysisStatus) {
    throw new Error("Connected Mac analysis status is unavailable.");
  }
  return window.terrane.healthAnalysisStatus(jobId);
}

export async function acknowledgeHealthAnalysis(jobId: string) {
  if (!window.terrane?.acknowledgeHealthAnalysis) {
    throw new Error("Connected Mac analysis acknowledgement is unavailable.");
  }
  return window.terrane.acknowledgeHealthAnalysis(jobId);
}

export async function pickPhoto(): Promise<PickResult> {
  if (!window.terrane || typeof window.terrane.pick !== "function") {
    throw new Error("The Photos picker is unavailable. Choose a file instead.");
  }
  return window.terrane.pick({
    source: "photos",
    types: ["image"],
    multiple: false,
  });
}

export function publishSidebar(
  selectedItemId: string,
  onSelect: (id: string) => void,
): void {
  const api = window.terrane;
  if (!api) return;
  api.setSidebarSection?.({
    title: "Health",
    selectedItemId,
    items: [
      { id: "add", title: "Add meal", systemImage: "plus.circle" },
      { id: "calendar", title: "Calendar", systemImage: "calendar" },
      {
        id: "history",
        title: "History",
        systemImage: "clock.arrow.circlepath",
      },
      { id: "insights", title: "Insights", systemImage: "chart.bar" },
      { id: "settings", title: "Settings", systemImage: "gearshape" },
    ],
  });
  api.onSidebarItemSelect?.(onSelect);
  api.onSidebarCreate?.(() => onSelect("add"));
}

export function fileToBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onerror = () =>
      reject(new Error("Could not read the selected image."));
    reader.onload = () => {
      const result = typeof reader.result === "string" ? reader.result : "";
      const comma = result.indexOf(",");
      if (comma < 0) {
        reject(new Error("Could not prepare the selected image."));
        return;
      }
      resolve(result.slice(comma + 1));
    };
    reader.readAsDataURL(file);
  });
}
