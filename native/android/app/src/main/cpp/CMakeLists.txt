cmake_minimum_required(VERSION 3.22.1)

project(native_ai_android_core LANGUAGES CXX)

add_library(zig_core_jni SHARED zig_core_jni.cpp)

find_library(log_lib log)

target_compile_features(zig_core_jni PRIVATE cxx_std_17)
target_link_libraries(zig_core_jni PRIVATE ${log_lib} dl)
