#include "webkit_host.h"

#include <string.h>

static gboolean native_ai_debug_build_allows_dev_flags(void) {
#ifndef NDEBUG
  return TRUE;
#else
  return FALSE;
#endif
}

static gboolean native_ai_is_forbidden_dev_flag(const char *argument) {
  static const char *flags[] = {
      "--control-plane-port",
      "--allow-runtime-mismatch",
      "--allow-unsigned-dev",
  };
  for (gsize index = 0; index < G_N_ELEMENTS(flags); index++) {
    const char *flag = flags[index];
    gsize flag_length = strlen(flag);
    if (g_strcmp0(argument, flag) == 0) {
      return TRUE;
    }
    if (g_str_has_prefix(argument, flag) && argument[flag_length] == '=') {
      return TRUE;
    }
  }
  return FALSE;
}

static gboolean native_ai_reject_dev_only_flags_if_needed(int argc, char **argv) {
  if (native_ai_debug_build_allows_dev_flags()) {
    return FALSE;
  }
  for (int index = 1; index < argc; index++) {
    if (native_ai_is_forbidden_dev_flag(argv[index])) {
      g_printerr("fatal: production build rejects dev-only startup flag %s\n", argv[index]);
      return TRUE;
    }
  }
  return FALSE;
}

static void on_activate(GtkApplication *application, gpointer user_data) {
  (void)user_data;
  WebKitHost *host = webkit_host_new(application);
  webkit_host_present(host);
  g_object_set_data_full(G_OBJECT(application), "native-ai-webapp-host", host, (GDestroyNotify)webkit_host_free);
}

int main(int argc, char **argv) {
  if (native_ai_reject_dev_only_flags_if_needed(argc, argv)) {
    return 1;
  }

  GtkApplication *application = gtk_application_new("dev.nativeai.webappplatform", G_APPLICATION_DEFAULT_FLAGS);
  g_signal_connect(application, "activate", G_CALLBACK(on_activate), NULL);
  int status = g_application_run(G_APPLICATION(application), argc, argv);
  g_object_unref(application);
  return status;
}
