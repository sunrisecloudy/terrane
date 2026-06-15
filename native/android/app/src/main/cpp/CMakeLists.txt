cmake_minimum_required(VERSION 3.22.1)

project(terrane_android_core LANGUAGES CXX)

add_library(forge_core_jni SHARED forge_core_jni.cpp)

find_library(log_lib log)

target_compile_features(forge_core_jni PRIVATE cxx_std_17)
target_link_libraries(forge_core_jni PRIVATE ${log_lib} dl)
