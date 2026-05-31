#pragma once

#include "web_bridge.h"

#include <glib.h>

typedef struct _DevControlPlane DevControlPlane;

typedef struct {
  guint requested_port;
  const gchar *database_path;
} DevControlPlaneConfig;

DevControlPlane *dev_control_plane_start(const DevControlPlaneConfig *config, GError **error);
void dev_control_plane_set_bridge(DevControlPlane *plane, WebBridge *bridge);
void dev_control_plane_stop(DevControlPlane *plane);
guint dev_control_plane_port(const DevControlPlane *plane);
const gchar *dev_control_plane_token_path(const DevControlPlane *plane);
