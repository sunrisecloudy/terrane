#include "platform_notifications.h"

static gboolean json_string_member(JsonObject *object, const gchar *member, const gchar **out) {
  if (object == NULL || !json_object_has_member(object, member)) {
    return FALSE;
  }
  JsonNode *node = json_object_get_member(object, member);
  if (!JSON_NODE_HOLDS_VALUE(node) || json_node_get_value_type(node) != G_TYPE_STRING) {
    return FALSE;
  }
  if (out != NULL) {
    *out = json_node_get_string(node);
  }
  return TRUE;
}

static gboolean valid_notification_level(const gchar *level) {
  return g_strcmp0(level, "info") == 0 || g_strcmp0(level, "success") == 0 ||
      g_strcmp0(level, "warning") == 0 || g_strcmp0(level, "error") == 0;
}

JsonNode *platform_notifications_toast(PlatformNotifications *notifications, const BridgeRequest *request) {
  (void)notifications;
  const gchar *message = NULL;
  if (!json_string_member(request->params, "message", &message)) {
    return bridge_failure(request, "invalid_request", "notification.toast requires message", NULL);
  }

  const gchar *level = NULL;
  if (json_object_has_member(request->params, "level")) {
    JsonNode *level_node = json_object_get_member(request->params, "level");
    if (json_node_get_node_type(level_node) != JSON_NODE_NULL && !json_string_member(request->params, "level", &level)) {
      return bridge_failure(request, "invalid_request", "notification.toast level must be a string", NULL);
    }
    if (level != NULL && !valid_notification_level(level)) {
      JsonObject *details = json_object_new();
      json_object_set_string_member(details, "level", level);
      return bridge_failure(
          request,
          "invalid_request",
          "notification.toast level must be info, success, warning, or error",
          details);
    }
  }

  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "ok");
  json_builder_add_boolean_value(builder, TRUE);
  json_builder_end_object(builder);
  return bridge_success(request, json_builder_get_root(builder));
}
