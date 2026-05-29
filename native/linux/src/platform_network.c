#include "platform_network.h"

#include <libsoup/soup.h>
#include <string.h>

typedef struct {
  gboolean valid;
  gchar *bytes;
  gsize len;
} RequestBody;

static gchar *origin_for_uri(GUri *uri) {
  const gchar *scheme = g_uri_get_scheme(uri);
  const gchar *host = g_uri_get_host(uri);
  if (scheme == NULL || host == NULL || (g_strcmp0(scheme, "http") != 0 && g_strcmp0(scheme, "https") != 0)) {
    return NULL;
  }
  gint port = g_uri_get_port(uri);
  g_autofree gchar *lower_host = g_ascii_strdown(host, -1);
  if (port > 0 && !((g_strcmp0(scheme, "http") == 0 && port == 80) || (g_strcmp0(scheme, "https") == 0 && port == 443))) {
    return g_strdup_printf("%s://%s:%d", scheme, lower_host, port);
  }
  return g_strdup_printf("%s://%s", scheme, lower_host);
}

static GHashTable *request_headers(JsonObject *params, gboolean *valid) {
  *valid = TRUE;
  GHashTable *headers = g_hash_table_new_full(g_str_hash, g_str_equal, g_free, g_free);
  if (!json_object_has_member(params, "headers") || json_object_get_null_member(params, "headers")) {
    return headers;
  }
  JsonObject *raw = json_object_get_object_member(params, "headers");
  if (raw == NULL) {
    *valid = FALSE;
    return headers;
  }

  GList *members = json_object_get_members(raw);
  for (GList *item = members; item != NULL; item = item->next) {
    const gchar *name = item->data;
    JsonNode *value = json_object_get_member(raw, name);
    if (json_node_get_value_type(value) != G_TYPE_STRING) {
      *valid = FALSE;
      break;
    }
    g_hash_table_insert(headers, g_ascii_strdown(name, -1), g_strdup(json_node_get_string(value)));
  }
  g_list_free(members);
  return headers;
}

static RequestBody request_body(JsonObject *params) {
  if (!json_object_has_member(params, "body") || json_object_get_null_member(params, "body")) {
    return (RequestBody){.valid = TRUE, .bytes = NULL, .len = 0};
  }
  JsonNode *value = json_object_get_member(params, "body");
  if (json_node_get_value_type(value) != G_TYPE_STRING) {
    return (RequestBody){.valid = FALSE};
  }
  const gchar *text = json_node_get_string(value);
  return (RequestBody){.valid = TRUE, .bytes = g_strdup(text), .len = strlen(text)};
}

static gboolean rule_allows(NetworkPolicyRule *rule, const gchar *origin, const gchar *method, GHashTable *headers) {
  if (g_strcmp0(rule->origin, origin) != 0 || !g_hash_table_contains(rule->methods, method)) {
    return FALSE;
  }
  GHashTableIter iter;
  gpointer key = NULL;
  g_hash_table_iter_init(&iter, headers);
  while (g_hash_table_iter_next(&iter, &key, NULL)) {
    const gchar *name = key;
    if (g_strcmp0(name, "cookie") == 0 || g_strcmp0(name, "set-cookie") == 0 || !g_hash_table_contains(rule->allowed_headers, name)) {
      return FALSE;
    }
  }
  return TRUE;
}

static NetworkPolicyRule *find_rule(GPtrArray *rules, const gchar *origin, const gchar *method, GHashTable *headers) {
  if (rules == NULL) {
    return NULL;
  }
  for (guint index = 0; index < rules->len; ++index) {
    NetworkPolicyRule *rule = g_ptr_array_index(rules, index);
    if (rule_allows(rule, origin, method, headers)) {
      return rule;
    }
  }
  return NULL;
}

static JsonObject *response_headers(SoupMessageHeaders *headers) {
  JsonObject *out = json_object_new();
  SoupMessageHeadersIter iter;
  const char *name = NULL;
  const char *value = NULL;
  soup_message_headers_iter_init(&iter, headers);
  while (soup_message_headers_iter_next(&iter, &name, &value)) {
    g_autofree gchar *lower = g_ascii_strdown(name, -1);
    json_object_set_string_member(out, lower, value);
  }
  return out;
}

static void append_request_header(gpointer key, gpointer value, gpointer user_data) {
  SoupMessageHeaders *headers = user_data;
  soup_message_headers_append(headers, key, value);
}

static gchar *absolute_redirect_url(const gchar *current_url, const gchar *location) {
  if (location == NULL || *location == '\0') {
    return NULL;
  }
  if (g_str_has_prefix(location, "http://") || g_str_has_prefix(location, "https://")) {
    return g_strdup(location);
  }
  if (g_str_has_prefix(location, "/")) {
    GError *error = NULL;
    GUri *uri = g_uri_parse(current_url, G_URI_FLAGS_NONE, &error);
    if (uri == NULL) {
      g_clear_error(&error);
      return NULL;
    }
    g_autofree gchar *origin = origin_for_uri(uri);
    g_uri_unref(uri);
    return origin == NULL ? NULL : g_strconcat(origin, location, NULL);
  }
  return NULL;
}

static gboolean parse_ipv4_host(const gchar *host, guint8 octets[4]) {
  const gchar *cursor = host;
  for (guint index = 0; index < 4; ++index) {
    guint value = 0;
    guint digits = 0;
    while (g_ascii_isdigit(*cursor)) {
      value = value * 10 + (guint)(*cursor - '0');
      if (value > 255) {
        return FALSE;
      }
      cursor++;
      digits++;
    }
    if (digits == 0) {
      return FALSE;
    }
    octets[index] = (guint8)value;
    if (index < 3) {
      if (*cursor != '.') {
        return FALSE;
      }
      cursor++;
    } else if (*cursor != '\0') {
      return FALSE;
    }
  }
  return TRUE;
}

static gboolean is_private_ipv4(guint8 octets[4]) {
  guint8 first = octets[0];
  guint8 second = octets[1];
  return first == 0 ||
      first == 10 ||
      first == 127 ||
      (first == 100 && second >= 64 && second <= 127) ||
      (first == 169 && second == 254) ||
      (first == 172 && second >= 16 && second <= 31) ||
      (first == 192 && second == 168);
}

static gboolean parse_hex16(const gchar *text, guint16 *out) {
  gsize len = strlen(text);
  if (len == 0 || len > 4) {
    return FALSE;
  }
  guint value = 0;
  for (const gchar *cursor = text; *cursor != '\0'; ++cursor) {
    gint digit = g_ascii_xdigit_value(*cursor);
    if (digit < 0) {
      return FALSE;
    }
    value = value * 16 + (guint)digit;
  }
  *out = (guint16)value;
  return TRUE;
}

static gboolean is_private_ipv4_mapped_host(const gchar *tail) {
  guint8 dotted[4] = {0};
  if (parse_ipv4_host(tail, dotted)) {
    return is_private_ipv4(dotted);
  }
  gchar **parts = g_strsplit(tail, ":", -1);
  if (g_strv_length(parts) != 2) {
    g_strfreev(parts);
    return FALSE;
  }
  guint16 high = 0;
  guint16 low = 0;
  if (!parse_hex16(parts[0], &high) || !parse_hex16(parts[1], &low)) {
    g_strfreev(parts);
    return FALSE;
  }
  g_strfreev(parts);
  guint8 octets[4] = {
      (guint8)(high >> 8),
      (guint8)(high & 0x00ff),
      (guint8)(low >> 8),
      (guint8)(low & 0x00ff),
  };
  return is_private_ipv4(octets);
}

static gboolean is_private_network_host(const gchar *raw_host) {
  if (raw_host == NULL || *raw_host == '\0') {
    return FALSE;
  }
  g_autofree gchar *lower = g_ascii_strdown(raw_host, -1);
  gchar *host = lower;
  gsize len = strlen(host);
  if (len >= 2 && host[0] == '[' && host[len - 1] == ']') {
    host[len - 1] = '\0';
    host++;
  }
  gchar *zone = strchr(host, '%');
  if (zone != NULL) {
    *zone = '\0';
  }
  if (g_strcmp0(host, "localhost") == 0 || g_str_has_suffix(host, ".localhost")) {
    return TRUE;
  }
  guint8 octets[4] = {0};
  if (parse_ipv4_host(host, octets)) {
    return is_private_ipv4(octets);
  }
  if (g_strcmp0(host, "::1") == 0) {
    return TRUE;
  }
  if (g_str_has_prefix(host, "fc") || g_str_has_prefix(host, "fd")) {
    return TRUE;
  }
  if (g_str_has_prefix(host, "fe8") || g_str_has_prefix(host, "fe9") || g_str_has_prefix(host, "fea") || g_str_has_prefix(host, "feb")) {
    return TRUE;
  }
  if (g_str_has_prefix(host, "::ffff:")) {
    return is_private_ipv4_mapped_host(host + strlen("::ffff:"));
  }
  return FALSE;
}

static JsonNode *network_failure(const BridgeRequest *request, const gchar *code, const gchar *message) {
  return bridge_failure(request, code, message, NULL);
}

JsonNode *platform_network_request(PlatformNetwork *network, const BridgeRequest *request) {
  (void)network;
  const gchar *url = json_object_get_string_member_with_default(request->params, "url", "");
  GError *error = NULL;
  GUri *uri = g_uri_parse(url, G_URI_FLAGS_NONE, &error);
  if (uri == NULL) {
    g_clear_error(&error);
    return network_failure(request, "invalid_request", "network.request requires an absolute http or https url");
  }
  g_autofree gchar *origin = origin_for_uri(uri);
  gboolean private_denied = request->context.deny_private_network && is_private_network_host(g_uri_get_host(uri));
  g_uri_unref(uri);
  if (origin == NULL) {
    return network_failure(request, "invalid_request", "network.request requires an absolute http or https url");
  }
  if (private_denied) {
    return network_failure(request, "network_policy_denied", "network.request private network targets are denied");
  }
  if (json_object_has_member(request->params, "credentials") && !json_object_get_null_member(request->params, "credentials")) {
    return network_failure(request, "network_policy_denied", "network.request credentials are not allowed");
  }

  g_autofree gchar *method = g_ascii_strup(json_object_get_string_member_with_default(request->params, "method", "GET"), -1);
  gboolean headers_valid = TRUE;
  GHashTable *headers = request_headers(request->params, &headers_valid);
  if (!headers_valid) {
    g_hash_table_unref(headers);
    return network_failure(request, "invalid_request", "network.request headers must be strings");
  }
  RequestBody body = request_body(request->params);
  if (!body.valid) {
    g_hash_table_unref(headers);
    return network_failure(request, "invalid_request", "network.request body must be a string or null");
  }

  NetworkPolicyRule *rule = find_rule(request->context.network_policy, origin, method, headers);
  if (rule == NULL) {
    g_free(body.bytes);
    g_hash_table_unref(headers);
    return network_failure(request, "network_policy_denied", "network.request is not allowed by manifest.networkPolicy");
  }
  if (body.len > rule->max_request_bytes) {
    g_free(body.bytes);
    g_hash_table_unref(headers);
    return network_failure(request, "network_policy_denied", "network.request body exceeds manifest.networkPolicy maxRequestBytes");
  }

  SoupSession *session = soup_session_new();
  g_object_set(session, "timeout", MAX(1, (guint)(rule->timeout_ms / 1000)), NULL);
  g_autofree gchar *current_url = g_strdup(url);
  for (guint redirects = 0; redirects < 6; ++redirects) {
    SoupMessage *message = soup_message_new(method, current_url);
    soup_message_set_flags(message, SOUP_MESSAGE_NO_REDIRECT);
    g_hash_table_foreach(headers, append_request_header, soup_message_get_request_headers(message));
    if (body.bytes != NULL) {
      GBytes *bytes = g_bytes_new(body.bytes, body.len);
      soup_message_set_request_body_from_bytes(message, "text/plain", bytes);
      g_bytes_unref(bytes);
    }

    GBytes *response = soup_session_send_and_read(session, message, NULL, &error);
    if (error != NULL) {
      const gchar *message_text = error->message == NULL ? "network.request failed" : error->message;
      JsonNode *failure = network_failure(request, "network_error", message_text);
      g_clear_error(&error);
      g_clear_object(&message);
      g_free(body.bytes);
      g_hash_table_unref(headers);
      g_object_unref(session);
      return failure;
    }

    guint status = soup_message_get_status(message);
    if (status >= 300 && status < 400) {
      const gchar *location = soup_message_headers_get_one(soup_message_get_response_headers(message), "Location");
      g_autofree gchar *next_url = absolute_redirect_url(current_url, location);
      g_clear_pointer(&response, g_bytes_unref);
      g_clear_object(&message);
      if (next_url == NULL) {
        g_free(body.bytes);
        g_hash_table_unref(headers);
        g_object_unref(session);
        return network_failure(request, "network_policy_denied", "network.request redirect is not allowed by manifest.networkPolicy");
      }
      GUri *next_uri = g_uri_parse(next_url, G_URI_FLAGS_NONE, NULL);
      g_autofree gchar *next_origin = next_uri == NULL ? NULL : origin_for_uri(next_uri);
      gboolean next_private_denied = request->context.deny_private_network && next_uri != NULL && is_private_network_host(g_uri_get_host(next_uri));
      if (next_uri != NULL) {
        g_uri_unref(next_uri);
      }
      NetworkPolicyRule *next_rule = next_origin == NULL ? NULL : find_rule(request->context.network_policy, next_origin, method, headers);
      if (next_private_denied || next_rule == NULL) {
        g_free(body.bytes);
        g_hash_table_unref(headers);
        g_object_unref(session);
        return network_failure(request, "network_policy_denied", "network.request redirect is not allowed by manifest.networkPolicy");
      }
      rule = next_rule;
      g_free(current_url);
      current_url = g_strdup(next_url);
      continue;
    }

    gsize response_len = 0;
    const gchar *response_data = response == NULL ? "" : g_bytes_get_data(response, &response_len);
    if (response_len > rule->max_response_bytes) {
      g_clear_pointer(&response, g_bytes_unref);
      g_clear_object(&message);
      g_free(body.bytes);
      g_hash_table_unref(headers);
      g_object_unref(session);
      return network_failure(request, "network_policy_denied", "network.response exceeds manifest.networkPolicy maxResponseBytes");
    }

    JsonBuilder *builder = json_builder_new();
    json_builder_begin_object(builder);
    json_builder_set_member_name(builder, "status");
    json_builder_add_int_value(builder, status);
    json_builder_set_member_name(builder, "headers");
    json_builder_add_value(builder, json_node_init_object(json_node_alloc(), response_headers(soup_message_get_response_headers(message))));
    json_builder_set_member_name(builder, "bodyText");
    g_autofree gchar *body_text = g_strndup(response_data, response_len);
    json_builder_add_string_value(builder, g_utf8_validate(body_text, -1, NULL) ? body_text : "");
    json_builder_end_object(builder);
    JsonNode *success = bridge_success(request, json_builder_get_root(builder));
    g_clear_pointer(&response, g_bytes_unref);
    g_clear_object(&message);
    g_free(body.bytes);
    g_hash_table_unref(headers);
    g_object_unref(session);
    return success;
  }

  g_free(body.bytes);
  g_hash_table_unref(headers);
  g_object_unref(session);
  return network_failure(request, "network_error", "network.request exceeded redirect limit");
}
