#include <jni.h>

#include <android/log.h>
#include <dlfcn.h>

#include <mutex>
#include <string>

namespace {

constexpr const char* kLogTag = "TerranePlatformCore";

using ForgeCoreOpenInMemoryFn = void* (*)(const char* workspace_id);
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

bool ensure_loaded_locked() {
  if (g_core != nullptr && g_handle_command != nullptr && g_free_string != nullptr) {
    return true;
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

  auto open_in_memory = reinterpret_cast<ForgeCoreOpenInMemoryFn>(dlsym(handle, "forge_core_open_in_memory"));
  auto handle_command = reinterpret_cast<ForgeCoreHandleCommandFn>(dlsym(handle, "forge_core_handle_command"));
  auto drain_events = reinterpret_cast<ForgeCoreDrainEventsFn>(dlsym(handle, "forge_core_drain_events"));
  auto last_error = reinterpret_cast<ForgeCoreLastErrorFn>(dlsym(handle, "forge_core_last_error"));
  auto close_core = reinterpret_cast<ForgeCoreCloseFn>(dlsym(handle, "forge_core_close"));
  auto free_string = reinterpret_cast<ForgeStringFreeFn>(dlsym(handle, "forge_string_free"));
  if (open_in_memory == nullptr ||
      handle_command == nullptr ||
      drain_events == nullptr ||
      last_error == nullptr ||
      close_core == nullptr ||
      free_string == nullptr) {
    __android_log_print(ANDROID_LOG_ERROR, kLogTag, "libforge_ffi.so is missing required forge_core_* symbols");
    dlclose(handle);
    return false;
  }

  void* core = open_in_memory("android-native");
  if (core == nullptr) {
    char* error = last_error();
    if (error != nullptr) {
      __android_log_print(ANDROID_LOG_ERROR, kLogTag, "forge_core_open_in_memory failed: %s", error);
      free_string(error);
    } else {
      __android_log_print(ANDROID_LOG_ERROR, kLogTag, "forge_core_open_in_memory returned null");
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
Java_com_terrane_platform_ForgeCoreBridge_nativeIsAvailable(JNIEnv*, jobject) {
  std::lock_guard<std::mutex> lock(g_core_mutex);
  return ensure_loaded_locked() ? JNI_TRUE : JNI_FALSE;
}

extern "C" JNIEXPORT jstring JNICALL
Java_com_terrane_platform_ForgeCoreBridge_nativeHandleCommand(JNIEnv* env, jobject, jstring command_json) {
  if (command_json == nullptr) {
    return nullptr;
  }

  std::lock_guard<std::mutex> lock(g_core_mutex);
  if (!ensure_loaded_locked()) {
    return nullptr;
  }

  const char* command_chars = env->GetStringUTFChars(command_json, nullptr);
  if (command_chars == nullptr) {
    return nullptr;
  }
  std::string command(command_chars);
  env->ReleaseStringUTFChars(command_json, command_chars);

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
