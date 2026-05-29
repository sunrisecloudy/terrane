cmake_minimum_required(VERSION 3.24)

project(NativeAIWebappHost LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

add_executable(NativeAIWebappHost WIN32
  src/main.cpp
  src/BridgeTypes.cpp
  src/WebViewHost.cpp
  src/WebBridge.cpp
  src/PlatformStorage.cpp
  src/PlatformDialogs.cpp
  src/PlatformNotifications.cpp
  src/PlatformNetwork.cpp
  src/ZigCoreBridge.cpp
)

target_compile_definitions(NativeAIWebappHost PRIVATE UNICODE _UNICODE NOMINMAX)
target_link_libraries(NativeAIWebappHost PRIVATE windowsapp winsqlite3 shell32 winhttp ole32)

set(NATIVE_AI_REPO_ROOT "${CMAKE_CURRENT_LIST_DIR}/../..")
add_custom_command(TARGET NativeAIWebappHost POST_BUILD
  COMMAND ${CMAKE_COMMAND} -E make_directory "$<TARGET_FILE_DIR:NativeAIWebappHost>/resources/runtime"
  COMMAND ${CMAKE_COMMAND} -E copy_directory "${NATIVE_AI_REPO_ROOT}/runtime-web" "$<TARGET_FILE_DIR:NativeAIWebappHost>/resources/runtime"
  COMMAND ${CMAKE_COMMAND} -E make_directory "$<TARGET_FILE_DIR:NativeAIWebappHost>/resources/webapps"
  COMMAND ${CMAKE_COMMAND} -E copy_directory "${NATIVE_AI_REPO_ROOT}/webapps/examples" "$<TARGET_FILE_DIR:NativeAIWebappHost>/resources/webapps/examples"
)
