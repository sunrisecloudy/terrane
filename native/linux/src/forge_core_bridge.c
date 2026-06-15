#include "forge_core_bridge.h"

#include <dlfcn.h>

typedef void *(*ForgeCoreOpenInMemoryFn)(const char *workspace_id);
typedef char *(*ForgeCoreHandleCommandFn)(void *core, const char *command_json);
typedef char *(*ForgeCoreDrainEventsFn)(void *core);
typedef char *(*ForgeCoreLastErrorFn)(void);
typedef void (*ForgeCoreCloseFn)(void *core);
typedef void (*ForgeStringFreeFn)(char *value);

static gchar **candidate_library_paths(void);
static gchar *executable_dir(void);
static gboolean load_library(ForgeCoreBridge *core, const gchar *path);
static JsonNode *core_payload_for_request(const BridgeRequest *request);
static JsonNode *core_command_for_request(const BridgeRequest *request);
static JsonNode *payload_from_core_response(ForgeCoreBridge *core, const BridgeRequest *request, gchar *output);
static gchar *json_node_to_string(JsonNode *node);
static void free_core_string(ForgeCoreBridge *core, gchar *value);

void forge_core_bridge_init(ForgeCoreBridge *core) {
  if (core == NULL) {
    return;
  }
  *core = (ForgeCoreBridge){0};

  gchar **paths = candidate_library_paths();
  for (gsize index = 0; paths[index] != NULL; ++index) {
    if (load_library(core, paths[index])) {
      break;
    }
  }
  g_strfreev(paths);
}

void forge_core_bridge_clear(ForgeCoreBridge *core) {
  if (core == NULL) {
    return;
  }
  ForgeCoreCloseFn close_core = (ForgeCoreCloseFn)core->close_core;
  if (close_core != NULL && core->core != NULL) {
    close_core(core->core);
  }
  if (core->handle != NULL) {
    dlclose(core->handle);
  }
  g_clear_pointer(&core->loaded_path, g_free);
  *core = (ForgeCoreBridge){0};
}

gboolean forge_core_bridge_is_available(const ForgeCoreBridge *core) {
  return core != NULL &&
      core->handle != NULL &&
      core->core != NULL &&
      core->handle_command != NULL &&
      core->free_string != NULL;
}

JsonNode *forge_core_bridge_step(ForgeCoreBridge *core, const BridgeRequest *request) {
  if (!forge_core_bridge_is_available(core)) {
    return bridge_failure(request, "platform_unsupported", "core.step requires loading libforge_ffi.so into the Linux host", NULL);
  }

  if (json_object_has_member(request->params, "app")) {
    JsonNode *app_node = json_object_get_member(request->params, "app");
    if (!JSON_NODE_HOLDS_VALUE(app_node) || json_node_get_value_type(app_node) != G_TYPE_STRING) {
      return bridge_failure(request, "invalid_request", "core.step app field must be a string when present", NULL);
    }
    const gchar *requested_app = json_node_get_string(app_node);
    if (g_strcmp0(requested_app, request->context.app_id) != 0) {
      JsonObject *details = json_object_new();
      json_object_set_string_member(details, "requestedApp", requested_app);
      json_object_set_string_member(details, "channelApp", request->context.app_id);
      return bridge_failure(request, "permission_denied", "core.step app field does not match the channel-derived app id", details);
    }
  }

  JsonNode *command_node = core_command_for_request(request);
  gchar *command_json = json_node_to_string(command_node);
  json_node_unref(command_node);

  ForgeCoreHandleCommandFn handle_command = (ForgeCoreHandleCommandFn)core->handle_command;
  gchar *output = handle_command(core->core, command_json);
  g_free(command_json);
  if (output == NULL) {
    return bridge_failure(request, "core_error", "forge_core_handle_command returned empty output", NULL);
  }
  return payload_from_core_response(core, request, output);
}

static gchar **candidate_library_paths(void) {
  GPtrArray *paths = g_ptr_array_new_with_free_func(g_free);
  const gchar *override_path = g_getenv("TERRANE_FORGE_FFI_SO");
  if (override_path != NULL && override_path[0] != '\0') {
    g_ptr_array_add(paths, g_strdup(override_path));
  }

  g_autofree gchar *dir = executable_dir();
  g_ptr_array_add(paths, g_build_filename(dir, "libforge_ffi.so", NULL));

  gchar *cwd = g_get_current_dir();
  g_ptr_array_add(paths, g_build_filename(cwd, "forge", "target", "debug", "libforge_ffi.so", NULL));
  g_ptr_array_add(paths, g_build_filename(cwd, "forge", "target", "release", "libforge_ffi.so", NULL));
  g_ptr_array_add(paths, g_build_filename(cwd, "..", "forge", "target", "debug", "libforge_ffi.so", NULL));
  g_ptr_array_add(paths, g_build_filename(cwd, "..", "forge", "target", "release", "libforge_ffi.so", NULL));
  g_ptr_array_add(paths, g_strdup("/usr/local/lib/libforge_ffi.so"));
  g_free(cwd);

  g_ptr_array_add(paths, NULL);
  return (gchar **)g_ptr_array_free(paths, FALSE);
}

static gchar *executable_dir(void) {
  g_autofree gchar *target = g_file_read_link("/proc/self/exe", NULL);
  if (target != NULL) {
    return g_path_get_dirname(target);
  }
  return g_get_current_dir();
}

static gboolean load_library(ForgeCoreBridge *core, const gchar *path) {
  void *handle = dlopen(path, RTLD_NOW | RTLD_LOCAL);
  if (handle == NULL) {
    return FALSE;
  }

  ForgeCoreOpenInMemoryFn open_in_memory = (ForgeCoreOpenInMemoryFn)dlsym(handle, "forge_core_open_in_memory");
  ForgeCoreHandleCommandFn handle_command = (ForgeCoreHandleCommandFn)dlsym(handle, "forge_core_handle_command");
  ForgeCoreDrainEventsFn drain_events = (ForgeCoreDrainEventsFn)dlsym(handle, "forge_core_drain_events");
  ForgeCoreLastErrorFn last_error = (ForgeCoreLastErrorFn)dlsym(handle, "forge_core_last_error");
  ForgeCoreCloseFn close_core = (ForgeCoreCloseFn)dlsym(handle, "forge_core_close");
  ForgeStringFreeFn free_string = (ForgeStringFreeFn)dlsym(handle, "forge_string_free");
  if (open_in_memory == NULL ||
      handle_command == NULL ||
      drain_events == NULL ||
      last_error == NULL ||
      close_core == NULL ||
      free_string == NULL) {
    dlclose(handle);
    return FALSE;
  }

  void *forge_core = open_in_memory("linux-native");
  if (forge_core == NULL) {
    gchar *error = last_error();
    if (error != NULL) {
      free_string(error);
    }
    dlclose(handle);
    return FALSE;
  }

  core->handle = handle;
  core->core = forge_core;
  core->close_core = close_core;
  core->handle_command = handle_command;
  core->drain_events = drain_events;
  core->last_error = last_error;
  core->free_string = free_string;
  core->loaded_path = g_strdup(path);
  return TRUE;
}

static JsonNode *core_payload_for_request(const BridgeRequest *request) {
  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);

  GList *members = json_object_get_members(request->params);
  for (GList *item = members; item != NULL; item = item->next) {
    const gchar *name = item->data;
    if (g_strcmp0(name, "app") == 0) {
      continue;
    }
    json_builder_set_member_name(builder, name);
    json_builder_add_value(builder, json_node_copy(json_object_get_member(request->params, name)));
  }
  g_list_free(members);

  json_builder_set_member_name(builder, "app");
  json_builder_add_string_value(builder, request->context.app_id);

  json_builder_end_object(builder);
  JsonNode *node = json_builder_get_root(builder);
  g_object_unref(builder);
  return node;
}

static JsonNode *core_command_for_request(const BridgeRequest *request) {
  JsonNode *payload = core_payload_for_request(request);
  JsonBuilder *builder = json_builder_new();
  json_builder_begin_object(builder);

  json_builder_set_member_name(builder, "request_id");
  json_builder_add_string_value(builder, request->has_id && request->id != NULL ? request->id : "linux-core-step");

  json_builder_set_member_name(builder, "actor");
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "actor");
  json_builder_add_string_value(builder, "linux-host");
  json_builder_set_member_name(builder, "role");
  json_builder_add_string_value(builder, "owner");
  json_builder_end_object(builder);

  json_builder_set_member_name(builder, "workspace_id");
  json_builder_add_string_value(builder, "linux-native");
  json_builder_set_member_name(builder, "name");
  json_builder_add_string_value(builder, "legacy.core_step");
  json_builder_set_member_name(builder, "payload");
  json_builder_add_value(builder, payload);

  json_builder_end_object(builder);
  JsonNode *node = json_builder_get_root(builder);
  g_object_unref(builder);
  return node;
}

static JsonNode *payload_from_core_response(ForgeCoreBridge *core, const BridgeRequest *request, gchar *output) {
  JsonParser *parser = json_parser_new();
  GError *parse_error = NULL;
  gboolean parsed = json_parser_load_from_data(parser, output, -1, &parse_error);
  free_core_string(core, output);
  if (!parsed) {
    g_clear_error(&parse_error);
    g_object_unref(parser);
    return bridge_failure(request, "core_error", "forge_core_handle_command returned invalid JSON", NULL);
  }

  JsonNode *root = json_parser_get_root(parser);
  JsonObject *response = root != NULL && JSON_NODE_HOLDS_OBJECT(root) ? json_node_get_object(root) : NULL;
  if (response == NULL || !json_object_has_member(response, "ok")) {
    g_object_unref(parser);
    return bridge_failure(request, "core_error", "forge_core_handle_command returned a malformed CoreResponse", NULL);
  }

  if (json_object_get_boolean_member(response, "ok")) {
    JsonNode *payload = json_object_has_member(response, "payload")
        ? json_node_copy(json_object_get_member(response, "payload"))
        : json_node_init_null(json_node_alloc());
    g_object_unref(parser);
    return bridge_success(request, payload);
  }

  JsonObject *details = json_object_new();
  json_object_set_member(details, "response", json_node_copy(root));
  g_object_unref(parser);
  return bridge_failure(request, "core_error", "legacy.core_step failed", details);
}

static gchar *json_node_to_string(JsonNode *node) {
  JsonGenerator *generator = json_generator_new();
  json_generator_set_root(generator, node);
  gchar *text = json_generator_to_data(generator, NULL);
  g_object_unref(generator);
  return text;
}

static void free_core_string(ForgeCoreBridge *core, gchar *value) {
  if (value == NULL || core == NULL || core->free_string == NULL) {
    return;
  }
  ForgeStringFreeFn free_string = (ForgeStringFreeFn)core->free_string;
  free_string(value);
}
