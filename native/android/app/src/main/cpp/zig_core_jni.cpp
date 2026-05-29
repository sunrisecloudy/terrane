#include <jni.h>

#include <android/log.h>
#include <dlfcn.h>
#include <stdint.h>

#include <mutex>
#include <string>

namespace {

constexpr const char* kLogTag = "NativeAIPlatformCore";

struct ZigCoreBuffer {
  uint8_t* ptr;
  size_t len;
};

using CoreCreateFn = void* (*)();
using CoreDestroyFn = void (*)(void*);
using CoreStepJsonFn = int32_t (*)(void*, const uint8_t*, size_t, ZigCoreBuffer*);
using CoreFreeFn = void (*)(ZigCoreBuffer);

std::mutex g_core_mutex;
bool g_load_attempted = false;
void* g_handle = nullptr;
void* g_core = nullptr;
CoreDestroyFn g_destroy = nullptr;
CoreStepJsonFn g_step_json = nullptr;
CoreFreeFn g_free_buffer = nullptr;

bool ensure_loaded_locked() {
  if (g_core != nullptr && g_step_json != nullptr && g_free_buffer != nullptr) {
    return true;
  }
  if (g_load_attempted) {
    return false;
  }
  g_load_attempted = true;

  void* handle = dlopen("libzig_core.so", RTLD_NOW | RTLD_LOCAL);
  if (handle == nullptr) {
    __android_log_print(ANDROID_LOG_ERROR, kLogTag, "dlopen libzig_core.so failed: %s", dlerror());
    return false;
  }

  auto create = reinterpret_cast<CoreCreateFn>(dlsym(handle, "core_create"));
  auto destroy = reinterpret_cast<CoreDestroyFn>(dlsym(handle, "core_destroy"));
  auto step_json = reinterpret_cast<CoreStepJsonFn>(dlsym(handle, "core_step_json"));
  auto free_buffer = reinterpret_cast<CoreFreeFn>(dlsym(handle, "core_free"));
  if (create == nullptr || destroy == nullptr || step_json == nullptr || free_buffer == nullptr) {
    __android_log_print(ANDROID_LOG_ERROR, kLogTag, "libzig_core.so is missing required core symbols");
    dlclose(handle);
    return false;
  }

  void* core = create();
  if (core == nullptr) {
    __android_log_print(ANDROID_LOG_ERROR, kLogTag, "core_create returned null");
    dlclose(handle);
    return false;
  }

  g_handle = handle;
  g_core = core;
  g_destroy = destroy;
  g_step_json = step_json;
  g_free_buffer = free_buffer;
  return true;
}

}  // namespace

extern "C" JNIEXPORT jboolean JNICALL
Java_com_nativeai_platform_ZigCoreBridge_nativeIsAvailable(JNIEnv*, jobject) {
  std::lock_guard<std::mutex> lock(g_core_mutex);
  return ensure_loaded_locked() ? JNI_TRUE : JNI_FALSE;
}

extern "C" JNIEXPORT jstring JNICALL
Java_com_nativeai_platform_ZigCoreBridge_nativeStep(JNIEnv* env, jobject, jstring input_json) {
  if (input_json == nullptr) {
    return nullptr;
  }

  std::lock_guard<std::mutex> lock(g_core_mutex);
  if (!ensure_loaded_locked()) {
    return nullptr;
  }

  const char* input_chars = env->GetStringUTFChars(input_json, nullptr);
  if (input_chars == nullptr) {
    return nullptr;
  }
  std::string input(input_chars);
  env->ReleaseStringUTFChars(input_json, input_chars);

  ZigCoreBuffer output{};
  const int32_t code = g_step_json(
      g_core,
      reinterpret_cast<const uint8_t*>(input.data()),
      input.size(),
      &output);
  if (code != 0 || output.ptr == nullptr) {
    return nullptr;
  }

  std::string output_text(reinterpret_cast<const char*>(output.ptr), output.len);
  g_free_buffer(output);
  return env->NewStringUTF(output_text.c_str());
}

extern "C" JNIEXPORT void JNICALL
JNI_OnUnload(JavaVM*, void*) {
  std::lock_guard<std::mutex> lock(g_core_mutex);
  if (g_destroy != nullptr && g_core != nullptr) {
    g_destroy(g_core);
  }
  if (g_handle != nullptr) {
    dlclose(g_handle);
  }
  g_handle = nullptr;
  g_core = nullptr;
  g_destroy = nullptr;
  g_step_json = nullptr;
  g_free_buffer = nullptr;
  g_load_attempted = false;
}
