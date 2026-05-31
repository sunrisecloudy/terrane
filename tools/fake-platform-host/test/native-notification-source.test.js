import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");

test("Linux and Windows native notification.toast validate fake-host contract params", () => {
  const windowsNotifications = read("native/windows/src/PlatformNotifications.cpp");
  const windowsHost = read("native/windows/src/WebViewHost.cpp");
  const linuxNotifications = read("native/linux/src/platform_notifications.c");
  const linuxHost = read("native/linux/src/webkit_host.c");

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
