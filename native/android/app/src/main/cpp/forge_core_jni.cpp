#include <jni.h>

#include <android/log.h>
#include <dlfcn.h>

#include <mutex>
#include <string>

namespace {

constexpr const char* kLogTag = "TerranePlatformCore";

using ForgeCoreOpenFn = void* (*)(const char* path, const char* workspace_id);
using ForgeCoreHandleCommandFn = char* (*)(void* core, const char* command_json);
using ForgeCoreDrainEventsFn = char* (*)(void* core);
using ForgeCoreLastErrorFn = char* (*)();
using ForgeCoreCloseFn = void (*)(void* core);
using ForgeStringFreeFn = void (*)(char* value);

std::mutex g_core_mutex;
bool g_load_attempted = false;
void* g_handle = nullptr;
void* g_core = nullptr;
ForgeCoreHandleCommandFn g_handle_command = nullptr;
ForgeCoreDrainEventsFn g_drain_events = nullptr;
ForgeCoreLastErrorFn g_last_error = nullptr;
ForgeCoreCloseFn g_close_core = nullptr;
ForgeStringFreeFn g_free_string = nullptr;

void free_core_string(char* value) {
  if (value != nullptr && g_free_string != nullptr) {
    g_free_string(value);
  }
}

std::string jni_string(JNIEnv* env, jstring value) {
  if (value == nullptr) {
    return {};
  }
  const char* chars = env->GetStringUTFChars(value, nullptr);
  if (chars == nullptr) {
    return {};
  }
  std::string text(chars);
  env->ReleaseStringUTFChars(value, chars);
  return text;
}

bool ensure_loaded_locked(const std::string& database_path) {
  if (g_core != nullptr && g_handle_command != nullptr && g_free_string != nullptr) {
    return true;
  }
  if (database_path.empty()) {
    __android_log_print(ANDROID_LOG_ERROR, kLogTag, "Forge database path is required");
    return false;
  }
  if (g_load_attempted) {
    return false;
  }
  g_load_attempted = true;

  void* handle = dlopen("libforge_ffi.so", RTLD_NOW | RTLD_LOCAL);
  if (handle == nullptr) {
    __android_log_print(ANDROID_LOG_ERROR, kLogTag, "dlopen libforge_ffi.so failed: %s", dlerror());
    return false;
  }

  auto open_core = reinterpret_cast<ForgeCoreOpenFn>(dlsym(handle, "forge_core_open"));
  auto handle_command = reinterpret_cast<ForgeCoreHandleCommandFn>(dlsym(handle, "forge_core_handle_command"));
  auto drain_events = reinterpret_cast<ForgeCoreDrainEventsFn>(dlsym(handle, "forge_core_drain_events"));
  auto last_error = reinterpret_cast<ForgeCoreLastErrorFn>(dlsym(handle, "forge_core_last_error"));
  auto close_core = reinterpret_cast<ForgeCoreCloseFn>(dlsym(handle, "forge_core_close"));
  auto free_string = reinterpret_cast<ForgeStringFreeFn>(dlsym(handle, "forge_string_free"));
  if (open_core == nullptr ||
      handle_command == nullptr ||
      drain_events == nullptr ||
      last_error == nullptr ||
      close_core == nullptr ||
      free_string == nullptr) {
    __android_log_print(ANDROID_LOG_ERROR, kLogTag, "libforge_ffi.so is missing required forge_core_* symbols");
    dlclose(handle);
    return false;
  }

  void* core = open_core(database_path.c_str(), "android-native");
  if (core == nullptr) {
    char* error = last_error();
    if (error != nullptr) {
      __android_log_print(ANDROID_LOG_ERROR, kLogTag, "forge_core_open failed: %s", error);
      free_string(error);
    } else {
      __android_log_print(ANDROID_LOG_ERROR, kLogTag, "forge_core_open returned null");
    }
    dlclose(handle);
    return false;
  }

  g_handle = handle;
  g_core = core;
  g_handle_command = handle_command;
  g_drain_events = drain_events;
  g_last_error = last_error;
  g_close_core = close_core;
  g_free_string = free_string;
  return true;
}

}  // namespace

extern "C" JNIEXPORT jboolean JNICALL
Java_com_terrane_platform_ForgeCoreBridge_nativeIsAvailable(JNIEnv* env, jobject, jstring database_path) {
  std::string path = jni_string(env, database_path);
  std::lock_guard<std::mutex> lock(g_core_mutex);
  return ensure_loaded_locked(path) ? JNI_TRUE : JNI_FALSE;
}

extern "C" JNIEXPORT jstring JNICALL
Java_com_terrane_platform_ForgeCoreBridge_nativeHandleCommand(JNIEnv* env, jobject, jstring database_path, jstring command_json) {
  if (database_path == nullptr || command_json == nullptr) {
    return nullptr;
  }

  std::string path = jni_string(env, database_path);
  std::string command = jni_string(env, command_json);
  if (path.empty() || command.empty()) {
    return nullptr;
  }

  std::lock_guard<std::mutex> lock(g_core_mutex);
  if (!ensure_loaded_locked(path)) {
    return nullptr;
  }

  char* output = g_handle_command(g_core, command.c_str());
  if (output == nullptr) {
    return nullptr;
  }

  std::string output_text(output);
  free_core_string(output);
  return env->NewStringUTF(output_text.c_str());
}

extern "C" JNIEXPORT void JNICALL
JNI_OnUnload(JavaVM*, void*) {
  std::lock_guard<std::mutex> lock(g_core_mutex);
  if (g_close_core != nullptr && g_core != nullptr) {
    g_close_core(g_core);
  }
  if (g_handle != nullptr) {
    dlclose(g_handle);
  }
  g_handle = nullptr;
  g_core = nullptr;
  g_handle_command = nullptr;
  g_drain_events = nullptr;
  g_last_error = nullptr;
  g_close_core = nullptr;
  g_free_string = nullptr;
  g_load_attempted = false;
}
