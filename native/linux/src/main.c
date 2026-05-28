#include "webkit_host.h"

static void on_activate(GtkApplication *application, gpointer user_data) {
  (void)user_data;
  WebKitHost *host = webkit_host_new(application);
  webkit_host_present(host);
  g_object_set_data_full(G_OBJECT(application), "native-ai-webapp-host", host, (GDestroyNotify)webkit_host_free);
}

int main(int argc, char **argv) {
  GtkApplication *application = gtk_application_new("dev.nativeai.webappplatform", G_APPLICATION_DEFAULT_FLAGS);
  g_signal_connect(application, "activate", G_CALLBACK(on_activate), NULL);
  int status = g_application_run(G_APPLICATION(application), argc, argv);
  g_object_unref(application);
  return status;
}
