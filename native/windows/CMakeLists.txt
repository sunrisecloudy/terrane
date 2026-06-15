cmake_minimum_required(VERSION 3.24)

project(TerraneHost LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

add_executable(TerraneHost WIN32
  src/main.cpp
  src/BridgeTypes.cpp
  src/DevControlPlane.cpp
  src/PlatformDatabase.cpp
  src/WebViewHost.cpp
  src/WebBridge.cpp
  src/PlatformStorage.cpp
  src/PlatformDialogs.cpp
  src/PlatformNotifications.cpp
  src/PlatformNetwork.cpp
  src/ForgeCoreBridge.cpp
)

target_compile_definitions(TerraneHost PRIVATE UNICODE _UNICODE NOMINMAX)
target_link_libraries(TerraneHost PRIVATE windowsapp winsqlite3 shell32 winhttp ole32 ws2_32 bcrypt)

if(WIN32)
  set(TERRANE_WEBVIEW2_NUGET_DIR "$ENV{TERRANE_WEBVIEW2_NUGET_DIR}" CACHE PATH "Expanded Microsoft.Web.WebView2 NuGet package")
  if(TERRANE_WEBVIEW2_NUGET_DIR)
    if(CMAKE_VS_PLATFORM_NAME MATCHES "ARM64")
      set(TERRANE_WEBVIEW2_ARCH arm64)
    elseif(CMAKE_SIZEOF_VOID_P EQUAL 8)
      set(TERRANE_WEBVIEW2_ARCH x64)
    else()
      set(TERRANE_WEBVIEW2_ARCH x86)
    endif()
    target_include_directories(TerraneHost PRIVATE "${TERRANE_WEBVIEW2_NUGET_DIR}/build/native/include")
    target_link_libraries(TerraneHost PRIVATE "${TERRANE_WEBVIEW2_NUGET_DIR}/build/native/${TERRANE_WEBVIEW2_ARCH}/WebView2LoaderStatic.lib")
  endif()
endif()

set(TERRANE_REPO_ROOT "${CMAKE_CURRENT_LIST_DIR}/../..")
set(TERRANE_FORGE_FFI_DLL "" CACHE FILEPATH "Optional prebuilt forge_ffi.dll to stage next to TerraneHost.exe")
add_custom_command(TARGET TerraneHost POST_BUILD
  COMMAND ${CMAKE_COMMAND} -E make_directory "$<TARGET_FILE_DIR:TerraneHost>/resources/runtime"
  COMMAND ${CMAKE_COMMAND} -E copy_directory "${TERRANE_REPO_ROOT}/runtime-web" "$<TARGET_FILE_DIR:TerraneHost>/resources/runtime"
  COMMAND ${CMAKE_COMMAND} -E make_directory "$<TARGET_FILE_DIR:TerraneHost>/resources/webapps"
  COMMAND ${CMAKE_COMMAND} -E copy_directory "${TERRANE_REPO_ROOT}/webapps/examples" "$<TARGET_FILE_DIR:TerraneHost>/resources/webapps/examples"
  COMMAND ${CMAKE_COMMAND} -E make_directory "$<TARGET_FILE_DIR:TerraneHost>/resources/db"
  COMMAND ${CMAKE_COMMAND} -E copy_directory "${TERRANE_REPO_ROOT}/db/sqlite" "$<TARGET_FILE_DIR:TerraneHost>/resources/db/sqlite"
)

if(TERRANE_FORGE_FFI_DLL)
  add_custom_command(TARGET TerraneHost POST_BUILD
    COMMAND ${CMAKE_COMMAND} -E copy_if_different "${TERRANE_FORGE_FFI_DLL}" "$<TARGET_FILE_DIR:TerraneHost>/forge_ffi.dll"
  )
endif()
