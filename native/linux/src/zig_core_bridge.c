#include "zig_core_bridge.h"

#include <dlfcn.h>
#include <stdint.h>
#include <string.h>

typedef struct ZigCore ZigCore;

typedef struct {
  uint8_t *ptr;
  size_t len;
} ZigCoreBuffer;

typedef ZigCore *(*CoreCreateFn)(void);
typedef void (*CoreDestroyFn)(ZigCore *core);
typedef int32_t (*CoreStepJsonFn)(ZigCore *core, const uint8_t *input_ptr, size_t input_len, ZigCoreBuffer *output);
typedef void (*CoreFreeFn)(ZigCoreBuffer buffer);

static gchar **candidate_library_paths(void);
static gchar *executable_dir(void);
static gboolean load_library(ZigCoreBridge *core, const gchar *path);
static JsonNode *core_input_for_request(const BridgeRequest *request);
static gchar *json_node_to_string(JsonNode *node);

void zig_core_bridge_init(ZigCoreBridge *core) {
  if (core == NULL) {
    return;
  }
  *core = (ZigCoreBridge){0};

  gchar **paths = candidate_library_paths();
  for (gsize index = 0; paths[index] != NULL; ++index) {
    if (load_library(core, paths[index])) {
      break;
    }
  }
  g_strfreev(paths);
}

void zig_core_bridge_clear(ZigCoreBridge *core) {
  if (core == NULL) {
    return;
  }
  CoreDestroyFn destroy = (CoreDestroyFn)core->destroy;
  if (destroy != NULL && core->core != NULL) {
    destroy((ZigCore *)core->core);
  }
  if (core->handle != NULL) {
    dlclose(core->handle);
  }
  g_clear_pointer(&core->loaded_path, g_free);
  *core = (ZigCoreBridge){0};
}

gboolean zig_core_bridge_is_available(const ZigCoreBridge *core) {
  return core != NULL && core->handle != NULL && core->core != NULL && core->step_json != NULL && core->free_buffer != NULL;
}

JsonNode *zig_core_bridge_step(ZigCoreBridge *core, const BridgeRequest *request) {
  if (!zig_core_bridge_is_available(core)) {
    return bridge_failure(request, "platform_unsupported", "core.step requires loading libzig_core.so into the Linux host", NULL);
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

  JsonNode *input_node = core_input_for_request(request);
  gchar *input_json = json_node_to_string(input_node);
  json_node_unref(input_node);

  ZigCoreBuffer output = {0};
  CoreStepJsonFn step_json = (CoreStepJsonFn)core->step_json;
  const int32_t code = step_json((ZigCore *)core->core, (const uint8_t *)input_json, strlen(input_json), &output);
  g_free(input_json);
  if (code != 0) {
    JsonObject *details = json_object_new();
    json_object_set_int_member(details, "status", code);
    return bridge_failure(request, "core_error", "core_step_json failed", details);
  }
  if (output.ptr == NULL) {
    return bridge_failure(request, "core_error", "core.step returned empty output", NULL);
  }

  JsonParser *parser = json_parser_new();
  GError *parse_error = NULL;
  gboolean parsed = json_parser_load_from_data(parser, (const gchar *)output.ptr, output.len, &parse_error);
  CoreFreeFn free_buffer = (CoreFreeFn)core->free_buffer;
  free_buffer(output);

  if (!parsed) {
    g_clear_error(&parse_error);
    g_object_unref(parser);
    return bridge_failure(request, "core_error", "core.step returned invalid JSON", NULL);
  }

  JsonNode *result = json_node_copy(json_parser_get_root(parser));
  g_object_unref(parser);
  return bridge_success(request, result);
}

static gchar **candidate_library_paths(void) {
  GPtrArray *paths = g_ptr_array_new_with_free_func(g_free);
  const gchar *override_path = g_getenv("NATIVE_AI_ZIG_CORE_SO");
  if (override_path != NULL && override_path[0] != '\0') {
    g_ptr_array_add(paths, g_strdup(override_path));
  }

  g_autofree gchar *dir = executable_dir();
  g_ptr_array_add(paths, g_build_filename(dir, "libzig_core.so", NULL));

  gchar *cwd = g_get_current_dir();
  g_ptr_array_add(paths, g_build_filename(cwd, "zig-core", "zig-out", "lib", "libzig_core.so", NULL));
  g_ptr_array_add(paths, g_build_filename(cwd, "..", "zig-core", "zig-out", "lib", "libzig_core.so", NULL));
  g_ptr_array_add(paths, g_strdup("/usr/local/lib/libzig_core.so"));
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

static gboolean load_library(ZigCoreBridge *core, const gchar *path) {
  void *handle = dlopen(path, RTLD_NOW | RTLD_LOCAL);
  if (handle == NULL) {
    return FALSE;
  }

  CoreCreateFn create = (CoreCreateFn)dlsym(handle, "core_create");
  CoreDestroyFn destroy = (CoreDestroyFn)dlsym(handle, "core_destroy");
  CoreStepJsonFn step_json = (CoreStepJsonFn)dlsym(handle, "core_step_json");
  CoreFreeFn free_buffer = (CoreFreeFn)dlsym(handle, "core_free");
  if (create == NULL || destroy == NULL || step_json == NULL || free_buffer == NULL) {
    dlclose(handle);
    return FALSE;
  }

  ZigCore *zig_core = create();
  if (zig_core == NULL) {
    dlclose(handle);
    return FALSE;
  }

  core->handle = handle;
  core->core = zig_core;
  core->destroy = destroy;
  core->step_json = step_json;
  core->free_buffer = free_buffer;
  core->loaded_path = g_strdup(path);
  return TRUE;
}

static JsonNode *core_input_for_request(const BridgeRequest *request) {
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

static gchar *json_node_to_string(JsonNode *node) {
  JsonGenerator *generator = json_generator_new();
  json_generator_set_root(generator, node);
  gchar *text = json_generator_to_data(generator, NULL);
  g_object_unref(generator);
  return text;
}
