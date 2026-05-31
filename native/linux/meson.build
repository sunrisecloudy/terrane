project('native-ai-webapp-host-linux', 'c',
  version: '0.1.0',
  default_options: ['warning_level=2', 'c_std=c17', 'b_ndebug=if-release'])

cc = meson.get_compiler('c')
gtk_dep = dependency('gtk4')
webkit_dep = dependency('webkitgtk-6.0')
json_dep = dependency('json-glib-1.0')
sqlite_dep = dependency('sqlite3')
soup_dep = dependency('libsoup-3.0')
dl_dep = cc.find_library('dl', required: false)

executable('native-ai-webapp-host',
  [
    'src/main.c',
    'src/app_sandbox.c',
    'src/bridge_types.c',
    'src/dev_control_plane.c',
    'src/platform_database.c',
    'src/webkit_host.c',
    'src/web_bridge.c',
    'src/platform_storage.c',
    'src/platform_dialogs.c',
    'src/platform_notifications.c',
    'src/platform_network.c',
    'src/zig_core_bridge.c',
  ],
  dependencies: [gtk_dep, webkit_dep, json_dep, sqlite_dep, soup_dep, dl_dep],
  install: true)
