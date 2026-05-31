import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");

test("Linux native notification.toast validates fake-host contract params", () => {
  const linuxNotifications = read("native/linux/src/platform_notifications.c");
  const linuxHost = read("native/linux/src/webkit_host.c");

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
