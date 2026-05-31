#include "PlatformDialogs.h"

#include <ShObjIdl.h>
#include <winrt/base.h>

#include <filesystem>
#include <fstream>
#include <optional>
#include <string>
#include <vector>

namespace nativeai {
namespace json = winrt::Windows::Data::Json;

namespace {
constexpr uint64_t kDefaultMaxFileBytes = 512 * 1024;

struct CoTaskMemString {
  PWSTR value = nullptr;

  ~CoTaskMemString() {
    if (value != nullptr) {
      CoTaskMemFree(value);
    }
  }
};

std::optional<std::filesystem::path> DialogPath(IShellItem* item) {
  if (item == nullptr) {
    return std::nullopt;
  }
  CoTaskMemString raw;
  if (FAILED(item->GetDisplayName(SIGDN_FILESYSPATH, &raw.value)) || raw.value == nullptr) {
    return std::nullopt;
  }
  return std::filesystem::path(raw.value);
}

struct OwnedFilterSpec {
  std::wstring name;
  std::wstring pattern;
};

std::wstring PatternForAccept(std::wstring const& accept) {
  if (accept == L"text/plain") {
    return L"*.txt";
  }
  if (accept == L"application/json") {
    return L"*.json";
  }
  if (accept == L"text/csv") {
    return L"*.csv";
  }
  if (accept == L"text/markdown") {
    return L"*.md;*.markdown";
  }
  if (accept == L"text/*") {
    return L"*.txt;*.md;*.markdown;*.csv;*.json";
  }
  if (accept == L"*/*") {
    return L"*.*";
  }
  if (accept.size() > 1 && accept[0] == L'.') {
    return L"*" + accept;
  }
  return L"*.*";
}

std::vector<OwnedFilterSpec> AcceptFilters(BridgeRequest const& request, bool& invalid) {
  invalid = false;
  std::vector<OwnedFilterSpec> filters;
  if (!request.params.HasKey(L"accept")) {
    return filters;
  }
  auto acceptValue = request.params.GetNamedValue(L"accept");
  if (acceptValue.ValueType() != json::JsonValueType::Array) {
    invalid = true;
    return filters;
  }
  auto accept = acceptValue.GetArray();
  for (uint32_t index = 0; index < accept.Size(); ++index) {
    auto item = accept.GetAt(index);
    if (item.ValueType() != json::JsonValueType::String) {
      invalid = true;
      filters.clear();
      return filters;
    }
    auto value = std::wstring(item.GetString().c_str());
    if (!value.empty()) {
      filters.push_back({value, PatternForAccept(value)});
    }
  }
  return filters;
}

std::vector<COMDLG_FILTERSPEC> DialogFilters(std::vector<OwnedFilterSpec> const& filters) {
  std::vector<COMDLG_FILTERSPEC> specs;
  specs.reserve(filters.size());
  for (auto const& filter : filters) {
    specs.push_back(COMDLG_FILTERSPEC{filter.name.c_str(), filter.pattern.c_str()});
  }
  return specs;
}

std::wstring MimeForPath(BridgeRequest const& request, std::filesystem::path const& path) {
  if (request.params.HasKey(L"accept")) {
    auto acceptValue = request.params.GetNamedValue(L"accept");
    if (acceptValue.ValueType() == json::JsonValueType::Array) {
      auto accept = acceptValue.GetArray();
      if (accept.Size() > 0 && accept.GetAt(0).ValueType() == json::JsonValueType::String) {
        return std::wstring(accept.GetAt(0).GetString().c_str());
      }
    }
  }
  auto extension = path.extension().wstring();
  if (extension == L".json") {
    return L"application/json";
  }
  return L"text/plain";
}

uint64_t MaxBytes(BridgeRequest const& request) {
  if (!request.params.HasKey(L"maxBytes")) {
    return kDefaultMaxFileBytes;
  }
  auto value = request.params.GetNamedValue(L"maxBytes");
  if (value.ValueType() != json::JsonValueType::Number) {
    return kDefaultMaxFileBytes;
  }
  auto number = value.GetNumber();
  return number <= 0 ? 0 : static_cast<uint64_t>(number);
}

bool BooleanParamIsTrue(BridgeRequest const& request, std::wstring const& name, bool& invalid) {
  invalid = false;
  winrt::hstring key{name};
  if (!request.params.HasKey(key)) {
    return false;
  }
  auto value = request.params.GetNamedValue(key);
  if (value.ValueType() != json::JsonValueType::Boolean) {
    invalid = true;
    return false;
  }
  return value.GetBoolean();
}

bool OptionalStringParamIsValid(BridgeRequest const& request, std::wstring const& name) {
  winrt::hstring key{name};
  return !request.params.HasKey(key) ||
         request.params.GetNamedValue(key).ValueType() == json::JsonValueType::String;
}

std::optional<std::string> ReadTextFile(std::filesystem::path const& path, uint64_t maxBytes) {
  std::ifstream file(path, std::ios::binary | std::ios::ate);
  if (!file) {
    return std::nullopt;
  }
  auto size = file.tellg();
  if (size < 0 || static_cast<uint64_t>(size) > maxBytes) {
    return std::nullopt;
  }
  file.seekg(0, std::ios::beg);
  std::string text(static_cast<size_t>(size), '\0');
  file.read(text.data(), static_cast<std::streamsize>(text.size()));
  if (!file && !file.eof()) {
    return std::nullopt;
  }
  return text;
}

bool WriteTextFile(std::filesystem::path const& path, std::wstring const& text) {
  std::ofstream file(path, std::ios::binary | std::ios::trunc);
  if (!file) {
    return false;
  }
  auto bytes = WideToUtf8(text);
  file.write(bytes.data(), static_cast<std::streamsize>(bytes.size()));
  return static_cast<bool>(file);
}

}  // namespace

PlatformDialogs::PlatformDialogs(HWND ownerWindow) : ownerWindow_(ownerWindow) {}

winrt::Windows::Data::Json::JsonObject PlatformDialogs::OpenFile(BridgeRequest const& request) {
  bool invalidMultiple = false;
  if (BooleanParamIsTrue(request, L"multiple", invalidMultiple)) {
    return BridgeResponse::Failure(request.id, request.hasId, L"platform_unsupported", L"dialog.openFile multiple selection is not supported on Windows yet");
  }
  if (invalidMultiple) {
    return BridgeResponse::Failure(request.id, request.hasId, L"invalid_request", L"dialog.openFile multiple must be a boolean");
  }
  bool invalidAccept = false;
  auto ownedFilters = AcceptFilters(request, invalidAccept);
  if (invalidAccept) {
    return BridgeResponse::Failure(request.id, request.hasId, L"invalid_request", L"dialog.openFile accept must be an array of strings");
  }
  auto filterSpecs = DialogFilters(ownedFilters);

  winrt::com_ptr<IFileOpenDialog> dialog;
  HRESULT hr = CoCreateInstance(CLSID_FileOpenDialog, nullptr, CLSCTX_INPROC_SERVER, IID_PPV_ARGS(dialog.put()));
  if (FAILED(hr)) {
    return BridgeResponse::Failure(request.id, request.hasId, L"platform_unsupported", L"dialog.openFile is unavailable");
  }

  DWORD options = 0;
  dialog->GetOptions(&options);
  dialog->SetOptions(options | FOS_FORCEFILESYSTEM | FOS_FILEMUSTEXIST);
  if (!filterSpecs.empty()) {
    dialog->SetFileTypes(static_cast<UINT>(filterSpecs.size()), filterSpecs.data());
  }

  hr = dialog->Show(ownerWindow_);
  if (hr == HRESULT_FROM_WIN32(ERROR_CANCELLED)) {
    return BridgeResponse::Failure(request.id, request.hasId, L"dialog_cancelled", L"Open file was cancelled");
  }
  if (FAILED(hr)) {
    return BridgeResponse::Failure(request.id, request.hasId, L"storage_error", L"Open file dialog failed");
  }

  winrt::com_ptr<IShellItem> item;
  if (FAILED(dialog->GetResult(item.put()))) {
    return BridgeResponse::Failure(request.id, request.hasId, L"storage_error", L"Open file result was unavailable");
  }
  auto path = DialogPath(item.get());
  if (!path.has_value()) {
    return BridgeResponse::Failure(request.id, request.hasId, L"storage_error", L"Open file path was unavailable");
  }

  auto text = ReadTextFile(path.value(), MaxBytes(request));
  if (!text.has_value()) {
    return BridgeResponse::Failure(request.id, request.hasId, L"quota_exceeded", L"Selected file could not be read within maxBytes");
  }

  json::JsonObject file;
  file.Insert(L"name", json::JsonValue::CreateStringValue(path->filename().wstring()));
  file.Insert(L"mime", json::JsonValue::CreateStringValue(MimeForPath(request, path.value())));
  file.Insert(L"size", json::JsonValue::CreateNumberValue(static_cast<double>(text->size())));
  file.Insert(L"text", json::JsonValue::CreateStringValue(Utf8ToWide(text.value())));

  json::JsonArray files;
  files.Append(file);

  json::JsonObject result;
  result.Insert(L"files", files);
  return BridgeResponse::Success(request.id, request.hasId, result);
}

winrt::Windows::Data::Json::JsonObject PlatformDialogs::SaveFile(BridgeRequest const& request) {
  if (!OptionalStringParamIsValid(request, L"suggestedName")) {
    return BridgeResponse::Failure(request.id, request.hasId, L"invalid_request", L"dialog.saveFile suggestedName must be a string");
  }
  if (!OptionalStringParamIsValid(request, L"mime")) {
    return BridgeResponse::Failure(request.id, request.hasId, L"invalid_request", L"dialog.saveFile mime must be a string");
  }
  if (!OptionalStringParamIsValid(request, L"text")) {
    return BridgeResponse::Failure(request.id, request.hasId, L"invalid_request", L"dialog.saveFile text must be a string");
  }

  winrt::com_ptr<IFileSaveDialog> dialog;
  HRESULT hr = CoCreateInstance(CLSID_FileSaveDialog, nullptr, CLSCTX_INPROC_SERVER, IID_PPV_ARGS(dialog.put()));
  if (FAILED(hr)) {
    return BridgeResponse::Failure(request.id, request.hasId, L"platform_unsupported", L"dialog.saveFile is unavailable");
  }

  auto suggestedName = std::wstring(request.params.GetNamedString(L"suggestedName", L"output.txt").c_str());
  dialog->SetFileName(suggestedName.c_str());

  hr = dialog->Show(ownerWindow_);
  if (hr == HRESULT_FROM_WIN32(ERROR_CANCELLED)) {
    return BridgeResponse::Failure(request.id, request.hasId, L"dialog_cancelled", L"Save file was cancelled");
  }
  if (FAILED(hr)) {
    return BridgeResponse::Failure(request.id, request.hasId, L"storage_error", L"Save file dialog failed");
  }

  winrt::com_ptr<IShellItem> item;
  if (FAILED(dialog->GetResult(item.put()))) {
    return BridgeResponse::Failure(request.id, request.hasId, L"storage_error", L"Save file result was unavailable");
  }
  auto path = DialogPath(item.get());
  if (!path.has_value()) {
    return BridgeResponse::Failure(request.id, request.hasId, L"storage_error", L"Save file path was unavailable");
  }

  auto text = std::wstring(request.params.GetNamedString(L"text", L"").c_str());
  if (!WriteTextFile(path.value(), text)) {
    return BridgeResponse::Failure(request.id, request.hasId, L"storage_error", L"Could not write selected file");
  }

  json::JsonObject result;
  result.Insert(L"ok", json::JsonValue::CreateBooleanValue(true));
  return BridgeResponse::Success(request.id, request.hasId, result);
}

}  // namespace nativeai
