import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");

test("native notification.toast implementations validate fake-host contract params", () => {
  const macosNotifications = read("native/macos/Sources/NativeAIHostMac/PlatformNotifications.swift");
  const iosNotifications = read("native/ios/Sources/NativeAIHostIOS/PlatformNotifications.swift");
  const androidNotifications = read("native/android/app/src/main/java/com/nativeai/platform/PlatformNotifications.kt");
  const windowsNotifications = read("native/windows/src/PlatformNotifications.cpp");
  const windowsHost = read("native/windows/src/WebViewHost.cpp");
  const linuxNotifications = read("native/linux/src/platform_notifications.c");
  const linuxHost = read("native/linux/src/webkit_host.c");

  for (const [label, source] of [
    ["macOS", macosNotifications],
    ["iOS", iosNotifications],
    ["Android", androidNotifications],
  ]) {
    assert.match(source, /notification\.toast requires message/, `${label} requires message`);
    assert.match(source, /notification\.toast level must be a string/, `${label} validates level type`);
    assert.match(source, /notification\.toast level must be info, success, warning, or error/, `${label} validates level enum`);
    assert.match(source, /"level"/, `${label} includes invalid level details`);
  }
  assert.match(macosNotifications, /validNotificationLevel\(level\)/);
  assert.match(iosNotifications, /validNotificationLevel\(level\)/);
  assert.match(androidNotifications, /level !in setOf\("info", "success", "warning", "error"\)/);
  assert.doesNotMatch(macosNotifications, /request\.params\["message"\] \?\? ""/);
  assert.doesNotMatch(iosNotifications, /request\.params\["message"\] \?\? ""/);
  assert.doesNotMatch(androidNotifications, /fun toast\(request: BridgeRequest\): String = BridgeResponse\.success/);

  assert.match(windowsNotifications, /notification\.toast requires message/);
  assert.match(windowsNotifications, /notification\.toast level must be a string/);
  assert.match(windowsNotifications, /notification\.toast level must be info, success, warning, or error/);
  assert.match(windowsNotifications, /details\.Insert\(L"level", json::JsonValue::CreateStringValue\(level\)\)/);
  assert.match(windowsNotifications, /ValidNotificationLevel\(level\)/);

  assert.match(windowsHost, /windows_smoke_fixed_notification_bad_level/);
  assert.match(windowsHost, /JsonResponseErrorCodeMatches\(notificationBadResponse, L"invalid_request"\)/);
  assert.match(windowsHost, /JsonResponseErrorDetailStringMatches\(notificationBadResponse, L"level", L"warn"\)/);
  assert.doesNotMatch(windowsHost, /notificationParams\.Insert\(L"title"/);
  assert.doesNotMatch(windowsHost, /notificationParams\.Insert\(L"body"/);

  assert.match(linuxNotifications, /notification\.toast requires message/);
  assert.match(linuxNotifications, /notification\.toast level must be a string/);
  assert.match(linuxNotifications, /notification\.toast level must be info, success, warning, or error/);
  assert.match(linuxNotifications, /json_object_set_string_member\(details, "level", level\)/);
  assert.match(linuxNotifications, /valid_notification_level\(level\)/);

  assert.match(linuxHost, /linux_smoke_fixed_notification_bad_level/);
  assert.match(linuxHost, /json_response_error_code_matches\(notification_bad_response, "invalid_request"\)/);
  assert.match(linuxHost, /json_response_error_detail_string_matches\(notification_bad_response, "level", "warn"\)/);
  assert.doesNotMatch(linuxHost, /json_builder_set_member_name\(notification_builder, "title"\)/);
  assert.doesNotMatch(linuxHost, /json_builder_set_member_name\(notification_builder, "body"\)/);
});

function read(relativePath) {
  return fs.readFileSync(path.join(repoRoot, relativePath), "utf8");
}
