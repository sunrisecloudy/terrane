#pragma once

#include <glib.h>
#include <sqlite3.h>

sqlite3 *platform_database_open(const gchar *database_path);
void platform_database_close(sqlite3 *db);
