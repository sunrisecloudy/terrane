import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");

test("native storage bridges enforce maxStorageBytes", () => {
  const iosStorage = read("native/ios/Sources/NativeAIHostIOS/PlatformStorage.swift");
  const macosStorage = read("native/macos/Sources/NativeAIHostMac/PlatformStorage.swift");
  const androidStorage = read("native/android/app/src/main/java/com/nativeai/platform/PlatformStorage.kt");
  const windowsStorage = read("native/windows/src/PlatformStorage.cpp");
  const windowsStorageHeader = read("native/windows/src/PlatformStorage.h");
  const linuxStorage = read("native/linux/src/platform_storage.c");

  for (const [target, source] of [
    ["ios", iosStorage],
    ["macos", macosStorage],
  ]) {
    assert.match(source, /request\.context\.resourceBudget\["maxStorageBytes"\]/, `${target} checks maxStorageBytes`);
    assert.match(source, /storageBytesAfterSet\(appId:/, `${target} projects storage bytes`);
    assert.match(source, /resource_budget_exceeded/, `${target} returns resource budget errors`);
    assert.match(source, /Storage write exceeds manifest\.resourceBudget\.maxStorageBytes/, `${target} names the budget`);
    assert.match(source, /"projectedBytes": projectedBytes/, `${target} reports projected bytes`);
  }

  assert.match(macosStorage, /code: "storage_error"/);
  assert.match(macosStorage, /message: "\\\(operation\) failed"/);
  assert.match(macosStorage, /guard sqlite3_prepare_v2\(db, sql, -1, &statement, nil\) == SQLITE_OK else/);
  assert.match(macosStorage, /guard sqlite3_step\(statement\) == SQLITE_DONE else/);

  assert.match(androidStorage, /request\.context\.resourceBudget\.optInt\("maxStorageBytes", -1\)/);
  assert.match(androidStorage, /private fun storageBytesAfterSet\(appId: String, key: String, valueBytes: Int\): Int/);
  assert.match(androidStorage, /"resource_budget_exceeded"/);
  assert.match(androidStorage, /"Storage write exceeds manifest\.resourceBudget\.maxStorageBytes"/);
  assert.match(androidStorage, /"projectedBytes" to projectedBytes/);

  assert.match(windowsStorageHeader, /StorageBytesAfterSet\(std::wstring const& appId, std::wstring const& key, int64_t valueBytes\) const/);
  assert.match(windowsStorage, /request\.context\.resourceBudget\.find\(L"maxStorageBytes"\)/);
  assert.match(windowsStorage, /int64_t PlatformStorage::StorageBytesAfterSet/);
  assert.match(windowsStorage, /L"resource_budget_exceeded"/);
  assert.match(windowsStorage, /L"Storage write exceeds manifest\.resourceBudget\.maxStorageBytes"/);
  assert.match(windowsStorage, /details\.Insert\(L"projectedBytes"/);

  assert.match(linuxStorage, /resource_budget_limit\(request, "maxStorageBytes", &limit\)/);
  assert.match(linuxStorage, /static gint64 storage_bytes_after_set/);
  assert.match(linuxStorage, /"resource_budget_exceeded"/);
  assert.match(linuxStorage, /"Storage write exceeds manifest\.resourceBudget\.maxStorageBytes"/);
  assert.match(linuxStorage, /json_object_set_int_member\(details, "projectedBytes", projected_bytes\)/);
});

test("macOS install transaction storage failures write failed install reports", () => {
  const macosControl = read("native/macos/Sources/NativeAIHostMac/DevControlPlane.swift");

  assert.match(macosControl, /recordInstallStorageFailureReport\(/);
  assert.match(macosControl, /INSERT OR REPLACE INTO app_install_reports/);
  assert.match(macosControl, /VALUES \(\?, \?, NULL, 'failed'/);
  assert.match(macosControl, /"code": "storage_error"/);
  assert.match(macosControl, /Package install transaction failed while writing platform storage/);
});

function read(relativePath) {
  return fs.readFileSync(path.join(repoRoot, relativePath), "utf8");
}
