/* Terrane host C ABI for non-Rust hosts.
 *
 * Hand-maintained (cbindgen is not a build dependency); the Rust test
 * `checked_in_c_header_declares_the_exported_abi` guards against drift.
 *
 * Memory: every char* written to out_output/out_error is owned by the caller and
 * must be freed exactly once with terrane_string_free (never free(3)). The
 * TerraneHandle* from terrane_open must be closed with terrane_close.
 */
#ifndef TERRANE_HOST_H
#define TERRANE_HOST_H

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

#define TERRANE_OK 0
#define TERRANE_ERR_NULL_ARG 1
#define TERRANE_ERR_UTF8 2
#define TERRANE_ERR_DISPATCH 3
#define TERRANE_ERR_PANIC 4
#define TERRANE_ERR_INTERNAL 5

typedef struct TerraneHandle TerraneHandle;

/* Open (or create) a workspace at `home` (dir holding log.bin); null/empty uses
 * the default. Returns a handle, or NULL on failure. */
TerraneHandle *terrane_open(const char *home);

/* Run an app backend using its manifest runtime. On success writes the
 * backend's output string to *out_output and returns TERRANE_OK; on failure
 * writes a message to *out_error and returns non-zero. */
int terrane_host_run(TerraneHandle *h, const char *app, size_t argc,
                     const char *const *argv, char **out_output, char **out_error);

/* Dispatch any command: name [args...]. On success writes committed event kinds
 * (one per line) to *out_output; on failure writes *out_error. */
int terrane_dispatch(TerraneHandle *h, const char *name, size_t argc,
                     const char *const *argv, char **out_output, char **out_error);

/* Create an in-memory App Builder preview from JSON:
 * {"files":[{"path":"manifest.json","content":"..."}, ...]}.
 * On success writes {"id":"...","frameUrl":"terrane-preview://<id>/frame/"}.
 */
int terrane_preview_create(TerraneHandle *h, const char *files_json,
                           char **out_output, char **out_error);

/* Read a preview asset. Empty path resolves to manifest.ui; non-empty path is
 * resolved relative to manifest.ui's parent. On success writes JSON with
 * content and contentType. */
int terrane_preview_read_asset(TerraneHandle *h, const char *preview_id,
                               const char *path, char **out_output,
                               char **out_error);
int terrane_preview_asset(TerraneHandle *h, const char *preview_id,
                          const char *path, char **out_output,
                          char **out_error);

/* Invoke a preview backend verb with string args. On success writes the
 * backend's returned output string. */
int terrane_preview_invoke(TerraneHandle *h, const char *preview_id,
                           const char *verb, size_t argc,
                           const char *const *argv, char **out_output,
                           char **out_error);

/* Generate a draft app through the core builder capability. On success writes
 * JSON with id/appId/name/prompt/harness/status/error/files. `harness` may be
 * "" to use the default app-generation harness. */
int terrane_builder_generate(TerraneHandle *h, const char *app_id,
                             const char *name, const char *prompt,
                             const char *harness, char **out_output,
                             char **out_error);

/* Build an app frontend with terrane-app-build. On success writes JSON with the
 * generated dist path and file count. */
int terrane_build_app(const char *app_dir, char **out_output, char **out_error);

/* Render the shared landing-page HTML for a host-supplied app catalog.
 * catalog_json: {"apps":[{"id":"...","name":"...","icon":"...","has_ui":true}, ...]}
 * (opaque text — the page's script parses it). app_href_template: per-app link
 * with an {id} placeholder, e.g. "terrane-app://{id}/frame/". Handle-free:
 * rendering reads nothing from the workspace. */
int terrane_home_page(const char *catalog_json, const char *app_href_template,
                      char **out_output, char **out_error);

/* Provision the MLX local-model runtime for the workspace at `home`
 * (null/empty = default home). Blocking: the first run may download the
 * runtime (~hundreds of MB). On success writes a human summary. Handle-free:
 * runtime provisioning is edge plumbing and records nothing in the event log. */
int terrane_local_model_setup_mlx(const char *home, char **out_output,
                                  char **out_error);

/* Resident mlx server status for the workspace at `home` as JSON:
 * {"running", "pid", "port", "idleSecs", "models"}. */
int terrane_local_model_server_status(const char *home, char **out_output,
                                      char **out_error);

/* Native ambient STT: open a capture session, enqueue PCM from a real-time
 * audio thread, then close. `sample_rate_hz` 0 defaults to 16000. */
int terrane_stt_session_begin(TerraneHandle *h, const char *app,
                              const char *session_id, unsigned sample_rate_hz);
int terrane_stt_push_pcm(const char *session_id, const short *pcm, size_t len);
int terrane_stt_session_end(TerraneHandle *h, const char *app,
                            const char *session_id, const char *reason);
void terrane_stt_shutdown(void);

/* Release in-process local-model inference engines. Call once before a normal
 * process exit (e.g. applicationWillTerminate); safe to call at any time. */
void terrane_local_model_shutdown(void);

/* Stop the resident mlx server for the workspace at `home`, if any. Writes a
 * short human summary. */
int terrane_local_model_server_stop(const char *home, char **out_output,
                                    char **out_error);

/* Resolve an RFC 7231 Accept-Language header (or any comma-joined preference
 * list) to the best supported Terrane language code, e.g. "fr-CH, en;q=0.8" ->
 * "fr". Writes the canonical code (e.g. "zh-Hans") or "en" when nothing
 * resolves. Pure: no handle required. */
int terrane_i18n_negotiate(const char *header, char **out_output,
                           char **out_error);

/* The canonical supported language codes as a JSON array, e.g.
 * ["en","es","zh-Hans",...] — one source of truth for native language pickers.
 * Pure: no handle required. */
int terrane_i18n_supported(char **out_output, char **out_error);

/* Import checked-in i18n catalogs (the "i18n/system" and "apps/<id>/i18n" JSON
 * files) under `path` into the workspace's public KV bucket via one
 * trusted-host kv.public.import. Idempotent and replay-safe. Writes a human
 * summary. */
int terrane_i18n_import(TerraneHandle *h, const char *path, char **out_output,
                        char **out_error);

/* The localized message bundle for `code` as a JSON object, to push to a UI.
 * `app_id` empty = the shell-chrome ("system") bundle; otherwise the app frame
 * bundle ("system" + that app's domain). English is the fallback layer; keys
 * are "<domain>.<key>" (e.g. "todo.add"). */
int terrane_i18n_bundle(TerraneHandle *h, const char *code, const char *app_id,
                        char **out_output, char **out_error);

/* Free a string returned by this library. Null-safe; non-null pointers are
 * single-use and must be freed exactly once. */
void terrane_string_free(char *s);

/* Close a handle from terrane_open. Null-safe; non-null handles are single-use
 * and must be closed exactly once. */
void terrane_close(TerraneHandle *h);

#ifdef __cplusplus
}
#endif

#endif /* TERRANE_HOST_H */
