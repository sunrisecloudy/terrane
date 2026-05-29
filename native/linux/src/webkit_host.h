#pragma once

#include "web_bridge.h"

#include <gtk/gtk.h>
#include <webkit/webkit.h>

typedef struct {
  GtkApplication *application;
  GtkWidget *window;
  WebKitWebView *web_view;
  WebBridge *bridge;
  gboolean smoke_ran;
} WebKitHost;

WebKitHost *webkit_host_new(GtkApplication *application);
void webkit_host_present(WebKitHost *host);
void webkit_host_free(WebKitHost *host);
