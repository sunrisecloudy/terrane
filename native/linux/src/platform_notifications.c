#include "platform_notifications.h"

JsonNode *platform_notifications_toast(PlatformNotifications *notifications, const BridgeRequest *request) {
  (void)notifications;
  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "ok");
  json_builder_add_boolean_value(builder, TRUE);
  json_builder_end_object(builder);
  return bridge_success(request, json_builder_get_root(builder));
}
