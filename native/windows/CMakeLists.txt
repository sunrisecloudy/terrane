cmake_minimum_required(VERSION 3.24)

project(NativeAIWebappHost LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

add_executable(NativeAIWebappHost WIN32
  src/main.cpp
  src/BridgeTypes.cpp
  src/PlatformDatabase.cpp
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

if(WIN32)
  set(NATIVE_AI_WEBVIEW2_NUGET_DIR "$ENV{NATIVE_AI_WEBVIEW2_NUGET_DIR}" CACHE PATH "Expanded Microsoft.Web.WebView2 NuGet package")
  if(NATIVE_AI_WEBVIEW2_NUGET_DIR)
    if(CMAKE_VS_PLATFORM_NAME MATCHES "ARM64")
      set(NATIVE_AI_WEBVIEW2_ARCH arm64)
    elseif(CMAKE_SIZEOF_VOID_P EQUAL 8)
      set(NATIVE_AI_WEBVIEW2_ARCH x64)
    else()
      set(NATIVE_AI_WEBVIEW2_ARCH x86)
    endif()
    target_include_directories(NativeAIWebappHost PRIVATE "${NATIVE_AI_WEBVIEW2_NUGET_DIR}/build/native/include")
    target_link_libraries(NativeAIWebappHost PRIVATE "${NATIVE_AI_WEBVIEW2_NUGET_DIR}/build/native/${NATIVE_AI_WEBVIEW2_ARCH}/WebView2LoaderStatic.lib")
  endif()
endif()

set(NATIVE_AI_REPO_ROOT "${CMAKE_CURRENT_LIST_DIR}/../..")
set(NATIVE_AI_ZIG_CORE_DLL "" CACHE FILEPATH "Optional prebuilt zig_core.dll to stage next to NativeAIWebappHost.exe")
add_custom_command(TARGET NativeAIWebappHost POST_BUILD
  COMMAND ${CMAKE_COMMAND} -E make_directory "$<TARGET_FILE_DIR:NativeAIWebappHost>/resources/runtime"
  COMMAND ${CMAKE_COMMAND} -E copy_directory "${NATIVE_AI_REPO_ROOT}/runtime-web" "$<TARGET_FILE_DIR:NativeAIWebappHost>/resources/runtime"
  COMMAND ${CMAKE_COMMAND} -E make_directory "$<TARGET_FILE_DIR:NativeAIWebappHost>/resources/webapps"
  COMMAND ${CMAKE_COMMAND} -E copy_directory "${NATIVE_AI_REPO_ROOT}/webapps/examples" "$<TARGET_FILE_DIR:NativeAIWebappHost>/resources/webapps/examples"
  COMMAND ${CMAKE_COMMAND} -E make_directory "$<TARGET_FILE_DIR:NativeAIWebappHost>/resources/db"
  COMMAND ${CMAKE_COMMAND} -E copy_directory "${NATIVE_AI_REPO_ROOT}/db/sqlite" "$<TARGET_FILE_DIR:NativeAIWebappHost>/resources/db/sqlite"
)

if(NATIVE_AI_ZIG_CORE_DLL)
  add_custom_command(TARGET NativeAIWebappHost POST_BUILD
    COMMAND ${CMAKE_COMMAND} -E copy_if_different "${NATIVE_AI_ZIG_CORE_DLL}" "$<TARGET_FILE_DIR:NativeAIWebappHost>/zig_core.dll"
  )
endif()
