#pragma once

#include "bridge_types.h"

#include <glib.h>

gboolean app_sandbox_is_known_example_app_id(const gchar *app_id);
gchar *app_sandbox_manifest_path_for_app(const gchar *app_id);
AppSandboxContext app_sandbox_context_for_app(const gchar *app_id, const gchar *mount_token);
